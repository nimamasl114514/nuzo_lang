// ============================================================================
// Control Stack Module - 循环控制流管理
// ============================================================================
//
// 本模块提供循环控制栈的完整实现，用于管理嵌套循环的 break/continue 跳转。
//
// ## 核心概念
//
// ### LoopContext（循环上下文）
// 每层循环对应一个 LoopContext，记录：
// - `start_ip`: 循环体起始位置（用于循环回跳）
// - `continue_ip`: continue 语句的目标位置（通常在增量操作处）
// - `break_patches`: 所有 break 语句的跳转指令位置（待回填）
// - `continue_patches`: 所有 continue 语句的跳转指令位置（待回填）
//
// ### ControlStack（控制栈）
// 使用 Vec<LoopContext> 实现的栈结构，支持嵌套循环。
// 提供 push/pop 操作以及高级封装方法 pop_and_prepare_patches。
//
// ### PatchInfo（修补信息）
// 从 pop_and_prepare_patches 返回的结构化数据，包含所有需要回填的跳转信息。
// 调用者使用此信息调用 patch_jump() 完成实际的字节码修补。
//
// ## 使用流程
//
// ```text
// 1. 进入循环 → control_stack.push_context(loop_start)
// 2. 遇到 break → 记录到 break_patches
// 3. 遇到 continue → 记录到 continue_patches
// 4. 更新 continue_ip → control_stack.last_mut().continue_ip = increment_ip
// 5. 退出循环 → control_stack.pop_and_prepare_patches(loop_end, line)?
// 6. 使用返回的 PatchInfo 调用 patch_jump() 完成回填
// ```
//
// ## 设计原则
//
// - **职责分离**: ControlStack 只管理数据，不直接修改字节码
// - **错误处理**: 栈下溢等边界情况返回 CompileError
// - **性能优化**: 使用 move 语义避免不必要的克隆
// - **API 友好**: 方法名显眼、符合 Rust 惯例、文档完善
// ============================================================================

use crate::compiler::CompileError;

// ============================================================================
// 数据结构定义 (Data Structures)
// ============================================================================

/// 循环上下文 - 控制栈的核心元素
///
/// 每进入一层循环（while/for/loop）就创建一个 LoopContext 并压入控制栈。
/// 它记录了该层循环的所有控制流信息，用于在循环结束时统一回填跳转目标。
///
/// # 字段说明
///
/// - `start_ip`: 循环体的起始字节码位置（用于无条件跳转回循环开头）
/// - `continue_ip`: continue 语句的目标位置（初始值等于 start_ip，可在循环体中更新）
/// - `break_patches`: break 语句产生的跳转指令位置列表（待回填为 loop_end）
/// - `continue_patches`: continue 语句产生的跳转指令位置列表（待回填为 continue_ip）
///
/// # 生命周期
///
/// 创建于循环入口，销毁于循环出口。生命周期与对应的 AST 循环节点一致。
#[derive(Debug)]
pub struct LoopContext {
    /// 循环体的起始字节码位置
    pub start_ip: usize,

    /// continue 语句的目标位置（通常在循环增量操作处）
    ///
    /// 对于 while/loop 循环，初始值等于 start_ip；
    /// 对于 for 循环，会在编译增量操作后更新为增量代码的位置。
    pub continue_ip: usize,

    /// break 语句的跳转指令位置列表（待回填）
    ///
    /// 每遇到一个 break 语句，就 emit 一个 Jmp 指令（offset=0），
    /// 并将其 ip 记录到此列表。循环结束时统一回填为 loop_end。
    pub break_patches: Vec<usize>,

    /// continue 语句的跳转指令位置列表（待回填）
    ///
    /// 每遇到一个 continue 语句，就 emit 一个 Jmp 指令（offset=0），
    /// 并将其 ip 记录到此列表。循环结束时统一回填为 continue_ip。
    pub continue_patches: Vec<usize>,
}

impl LoopContext {
    /// 创建新的循环上下文
    ///
    /// # 参数
    ///
    /// - `start_ip`: 循环体的起始字节码位置
    ///
    /// # 返回值
    ///
    /// 返回一个初始化好的 LoopContext，其中：
    /// - `start_ip` 和 `continue_ip` 都设置为传入的值
    /// - `break_patches` 和 `continue_patches` 为空 Vec
    #[inline]
    pub fn new(start_ip: usize) -> Self {
        Self {
            start_ip,
            continue_ip: start_ip,
            break_patches: Vec::new(),
            continue_patches: Vec::new(),
        }
    }
}

// ============================================================================
// 控制栈实现 (Control Stack Implementation)
// ============================================================================

/// 控制栈 - 管理嵌套循环的 break/continue 跳转
///
/// 使用 `Vec<LoopContext>` 实现的后进先出（LIFO）栈结构。
/// 支持任意深度的嵌套循环，每层循环对应一个栈帧。
///
/// # 线程安全
///
/// 本类型不是线程安全的（内部使用 Vec），但编译器本身是单线程的，
/// 所以这不构成问题。
///
/// # 典型用法
///
/// ```ignore
/// // 进入循环
/// control_stack.push_context(loop_start);
///
/// // ... 编译循环体（可能遇到 break/continue） ...
///
/// // 更新 continue 目标（for 循环的增量操作）
/// if let Some(ctx) = control_stack.last_mut() {
///     ctx.continue_ip = increment_ip;
/// }
///
/// // 退出循环并获取修补信息
/// let patch_info = control_stack.pop_and_prepare_patches(loop_end, line)?;
///
/// // 使用 patch_info 回填所有跳转指令
/// // （这部分由调用者完成，不在 ControlStack 的职责范围内）
/// ```
#[derive(Debug)]
pub struct ControlStack {
    /// 内部存储：循环上下文向量
    stack: Vec<LoopContext>,
}

impl ControlStack {
    /// 创建新的空控制栈
    ///
    /// # 返回值
    ///
    /// 返回一个不包含任何元素的 ControlStack。
    /// 通常在 Compiler 初始化时调用。
    ///
    /// # 示例
    ///
    /// ```ignore
    /// let mut control_stack = ControlStack::new();
    /// assert!(control_stack.is_empty());
    /// assert_eq!(control_stack.depth(), 0);
    /// ```
    #[inline]
    pub fn new() -> Self {
        Self { stack: Vec::new() }
    }

    /// 压入一个新的循环上下文
    ///
    /// 在进入循环（while/for/loop）时调用。
    /// 创建一个新的 LoopContext 并压入栈顶。
    ///
    /// # 参数
    ///
    /// - `start_ip`: 当前字节码长度，作为循环体的起始位置
    ///
    /// # 副作用
    ///
    /// 修改内部栈，将新元素压入栈顶。
    ///
    /// # 示例
    ///
    /// ```ignore
    /// let loop_start = chunk.code().len();
    /// control_stack.push_context(loop_start);
    /// ```
    #[inline]
    pub fn push_context(&mut self, start_ip: usize) {
        self.stack.push(LoopContext::new(start_ip));
    }

    /// 弹出栈顶的循环上下文
    ///
    /// 在退出循环时调用。弹出并返回栈顶的 LoopContext。
    ///
    /// # 错误
    ///
    /// 如果控制栈为空（栈下溢），返回 `CompileError::Error`，
    /// 错误消息为 "loop stack underflow"。
    ///
    /// # 返回值
    ///
    /// - 成功: 返回弹出的 `LoopContext`（通过 move 语义转移所有权）
    /// - 失败: 返回 `CompileError`
    ///
    /// # 注意
    ///
    /// 大多数情况下应该使用 `pop_and_prepare_patches()` 高级方法，
    /// 除非你需要原始的 LoopContext 数据。
    pub fn pop_context(&mut self, line: usize) -> Result<LoopContext, CompileError> {
        self.stack.pop().ok_or_else(|| CompileError::Error {
            message: "loop stack underflow".to_string(),
            line,
            column: 0,
        })
    }

    /// 获取栈顶元素的可变引用
    ///
    /// 用于更新当前循环的 `continue_ip`（例如 for 循环的增量操作位置）。
    ///
    /// # 返回值
    ///
    /// - `Some(&mut LoopContext)`: 如果栈不为空
    /// - `None`: 如果栈为空
    ///
    /// # 示例
    ///
    /// ```ignore
    /// // 在 for 循环的增量操作后更新 continue 目标
    /// let increment_ip = chunk.code().len();
    /// if let Some(ctx) = control_stack.last_mut() {
    ///     ctx.continue_ip = increment_ip;
    /// }
    /// ```
    #[inline]
    pub fn last_mut(&mut self) -> Option<&mut LoopContext> {
        self.stack.last_mut()
    }

    /// 获取栈顶元素的不可变引用
    ///
    /// 用于读取当前循环的信息（不修改）。
    ///
    /// # 返回值
    ///
    /// - `Some(&LoopContext)`: 如果栈不为空
    /// - `None`: 如果栈为空
    #[inline]
    pub fn last(&self) -> Option<&LoopContext> {
        self.stack.last()
    }

    /// 检查控制栈是否为空
    ///
    /// # 返回值
    ///
    /// - `true`: 栈为空（当前不在任何循环中）
    /// - `false`: 栈不为空（当前在一个或多个嵌套循环中）
    ///
    /// # 用途
    ///
    /// 用于验证 break/continue 是否在循环内使用。
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.stack.is_empty()
    }

    /// 获取当前嵌套深度
    ///
    /// # 返回值
    ///
    /// 返回当前嵌套的循环层数。
    /// - 0 表示不在循环中
    /// - 1 表示在单层循环中
    /// - N > 1 表示在 N 层嵌套循环中
    #[inline]
    pub fn depth(&self) -> usize {
        self.stack.len()
    }

    /// 弹出栈顶循环上下文并准备所有修补数据（高级封装方法）
    ///
    /// 这是退出循环时的推荐方法。它：
    /// 1. 弹出栈顶的 LoopContext
    /// 2. 将其转换为结构化的 `PatchInfo`
    /// 3. 返回给调用者进行实际的字节码修补
    ///
    /// # 参数
    ///
    /// - `loop_end`: 循环结束位置（通常是当前 code.len()）
    /// - `current_line`: 当前行号（用于错误报告）
    ///
    /// # 返回值
    ///
    /// - 成功: 返回 `PatchInfo`，包含所有需要回填的跳转信息
    /// - 失败: 返回 `CompileError`（栈下溢等情况）
    ///
    /// # 设计理念
    ///
    /// 本方法**不直接调用 patch_jump**，而是返回需要修补的数据。
    /// 这样的设计遵循单一职责原则：
    /// - ControlStack 负责管理控制流数据
    /// - Compiler/helpers 负责实际修改字节码
    ///
    /// # 典型用法
    ///
    /// ```ignore
    /// // 退出循环
    /// let patch_info = control_stack.pop_and_prepare_patches(
    ///     chunk.code().len(),  // loop_end
    ///     current_line       // 用于错误报告
    /// )?;
    ///
    /// // 先修补条件跳转（test_ip -> loop_end）
    /// compiler.patch_jump(test_ip, patch_info.loop_end)?;
    ///
    /// // 修补所有 break 跳转（-> loop_end）
    /// for &break_ip in &patch_info.break_patches {
    ///     compiler.patch_jump(break_ip, patch_info.loop_end)?;
    /// }
    ///
    /// // 修补所有 continue 跳转（-> continue_target）
    /// for &cont_ip in &patch_info.continue_patches {
    ///     compiler.patch_jump(cont_ip, patch_info.continue_target)?;
    /// }
    /// ```
    pub fn pop_and_prepare_patches(
        &mut self,
        loop_end: usize,
        current_line: usize,
    ) -> Result<PatchInfo, CompileError> {
        let ctx = self.pop_context(current_line)?;

        Ok(PatchInfo {
            loop_end,
            break_patches: ctx.break_patches,
            continue_patches: ctx.continue_patches,
            continue_target: ctx.continue_ip,
        })
    }
}

// 为 ControlStack 提供 Default trait 实现
impl Default for ControlStack {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// 修补信息结构 (Patch Information)
// ============================================================================

/// 修补信息 - 从 pop_and_prepare_patches 返回的结构化数据
///
/// 包含退出循环时需要的所有跳转回填信息。
/// 调用者使用此结构调用 `patch_jump()` 完成实际的字节码修改。
///
/// # 字段说明
///
/// - `loop_end`: 循环结束位置（break 和条件跳转的目标）
/// - `break_patches`: 所有 break 跳转指令的位置列表
/// - `continue_patches`: 所有 continue 跳转指令的位置列表
/// - `continue_target`: continue 语句的目标位置（来自 LoopContext.continue_ip）
///
/// # 所有权
///
/// 所有 Vec 字段都通过 move 转移所有权，避免不必要的克隆。
/// 这符合 Rust 的零成本抽象原则。
#[derive(Debug)]
pub struct PatchInfo {
    /// 循环结束位置（break 和条件失败跳转的目标）
    pub loop_end: usize,

    /// break 语句的跳转指令位置列表（每个都需要回填为 loop_end）
    pub break_patches: Vec<usize>,

    /// continue 语句的跳转指令位置列表（每个都需要回填为 continue_target）
    pub continue_patches: Vec<usize>,

    /// continue 语句的目标位置（通常在增量操作处）
    pub continue_target: usize,
}

impl PatchInfo {
    /// 获取需要修补的跳转总数
    ///
    /// # 返回值
    ///
    /// return break_patches.len() + continue_patches.len()
    #[inline]
    pub fn total_patches(&self) -> usize {
        self.break_patches.len() + self.continue_patches.len()
    }

    /// 检查是否有需要修补的跳转
    ///
    /// # 返回值
    ///
    /// - `true`: 如果存在至少一个 break 或 continue 跳转需要修补
    /// - `false`: 如果没有任何跳转需要修补（循环体内无 break/continue）
    #[inline]
    pub fn has_patches(&self) -> bool {
        !self.break_patches.is_empty() || !self.continue_patches.is_empty()
    }
}

// ============================================================================
// 单元测试 (Unit Tests)
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_control_stack_new() {
        let stack = ControlStack::new();
        assert!(stack.is_empty());
        assert_eq!(stack.depth(), 0);
    }

    #[test]
    fn test_push_and_pop() {
        let mut stack = ControlStack::new();

        // 压入第一个循环
        stack.push_context(100);
        assert_eq!(stack.depth(), 1);
        assert!(!stack.is_empty());

        // 压入第二个循环（嵌套）
        stack.push_context(200);
        assert_eq!(stack.depth(), 2);

        // 弹出第二个循环
        let ctx = stack.pop_context(0).unwrap();
        assert_eq!(ctx.start_ip, 200);
        assert_eq!(stack.depth(), 1);

        // 弹出第一个循环
        let ctx = stack.pop_context(0).unwrap();
        assert_eq!(ctx.start_ip, 100);
        assert!(stack.is_empty());
    }

    #[test]
    fn test_pop_underflow() {
        let mut stack = ControlStack::new();

        // 尝试从空栈弹出应该报错
        let result = stack.pop_context(10);
        assert!(result.is_err());

        if let Err(CompileError::Error { message, line, .. }) = result {
            assert_eq!(message, "loop stack underflow");
            assert_eq!(line, 10);
        } else {
            panic!("Expected CompileError::Error");
        }
    }

    #[test]
    fn test_last_mut() {
        let mut stack = ControlStack::new();

        // 空栈时 last_mut 返回 None
        assert!(stack.last_mut().is_none());

        // 压入后可以获取可变引用
        stack.push_context(100);
        {
            let ctx = stack.last_mut().unwrap();
            assert_eq!(ctx.start_ip, 100);
            assert_eq!(ctx.continue_ip, 100);

            // 修改 continue_ip
            ctx.continue_ip = 150;
        }

        // 验证修改生效
        let ctx = stack.last().unwrap();
        assert_eq!(ctx.continue_ip, 150);
    }

    #[test]
    fn test_pop_and_prepare_patches() {
        let mut stack = ControlStack::new();

        // 模拟循环编译过程
        stack.push_context(100);

        // 模拟记录一些 break 和 continue
        {
            let ctx = stack.last_mut().unwrap();
            ctx.break_patches.push(120); // break at ip 120
            ctx.break_patches.push(140); // break at ip 140
            ctx.continue_patches.push(130); // continue at ip 130
            ctx.continue_ip = 150; // 更新 continue 目标
        }

        // 弹出并准备修补信息
        let patch_info = stack.pop_and_prepare_patches(200, 42).unwrap();

        // 验证 PatchInfo 内容
        assert_eq!(patch_info.loop_end, 200);
        assert_eq!(patch_info.break_patches, vec![120, 140]);
        assert_eq!(patch_info.continue_patches, vec![130]);
        assert_eq!(patch_info.continue_target, 150);

        // 验证辅助方法
        assert_eq!(patch_info.total_patches(), 3);
        assert!(patch_info.has_patches());

        // 栈应该为空
        assert!(stack.is_empty());
    }

    #[test]
    fn test_nested_loops() {
        let mut stack = ControlStack::new();

        // 模拟三层嵌套循环
        stack.push_context(100); // 外层
        stack.push_context(200); // 中层
        stack.push_context(300); // 内层

        assert_eq!(stack.depth(), 3);

        // 弹出内层
        let inner = stack.pop_context(0).unwrap();
        assert_eq!(inner.start_ip, 300);
        assert_eq!(stack.depth(), 2);

        // 弹出中层
        let middle = stack.pop_context(0).unwrap();
        assert_eq!(middle.start_ip, 200);
        assert_eq!(stack.depth(), 1);

        // 弹出外层
        let outer = stack.pop_context(0).unwrap();
        assert_eq!(outer.start_ip, 100);
        assert!(stack.is_empty());
    }

    #[test]
    fn test_loop_context_default_values() {
        let ctx = LoopContext::new(100);

        assert_eq!(ctx.start_ip, 100);
        assert_eq!(ctx.continue_ip, 100); // 默认等于 start_ip
        assert!(ctx.break_patches.is_empty());
        assert!(ctx.continue_patches.is_empty());
    }

    #[test]
    fn test_patch_info_helpers() {
        let patch_info = PatchInfo {
            loop_end: 200,
            break_patches: vec![],
            continue_patches: vec![],
            continue_target: 150,
        };

        // 空 patches
        assert_eq!(patch_info.total_patches(), 0);
        assert!(!patch_info.has_patches());

        // 有 patches
        let patch_info_with_patches = PatchInfo {
            loop_end: 200,
            break_patches: vec![120, 140],
            continue_patches: vec![130],
            continue_target: 150,
        };

        assert_eq!(patch_info_with_patches.total_patches(), 3);
        assert!(patch_info_with_patches.has_patches());
    }

    // ========================================================================
    // 显式覆盖测试：ControlStack 与 PatchInfo 公共 API
    // ========================================================================

    #[test]
    fn test_push_context_increases_depth() {
        let mut stack = ControlStack::new();
        assert_eq!(stack.depth(), 0);

        stack.push_context(100);
        assert_eq!(stack.depth(), 1);

        stack.push_context(200);
        assert_eq!(stack.depth(), 2);

        // 验证栈顶内容
        let top = stack.last().unwrap();
        assert_eq!(top.start_ip, 200);
        assert_eq!(top.continue_ip, 200); // 默认等于 start_ip
    }

    #[test]
    fn test_pop_context_returns_inserted_context() {
        let mut stack = ControlStack::new();
        stack.push_context(42);

        let ctx = stack.pop_context(0).unwrap();
        assert_eq!(ctx.start_ip, 42);
        assert_eq!(ctx.continue_ip, 42);
        assert!(ctx.break_patches.is_empty());
        assert!(ctx.continue_patches.is_empty());
        assert!(stack.is_empty());
    }

    #[test]
    fn test_pop_context_underflow_returns_error() {
        let mut stack = ControlStack::new();
        let result = stack.pop_context(99);
        assert!(result.is_err());
        match result {
            Err(CompileError::Error { message, line, .. }) => {
                assert_eq!(message, "loop stack underflow");
                assert_eq!(line, 99);
            }
            _ => panic!("Expected CompileError::Error"),
        }
    }

    #[test]
    fn test_has_patches_detects_break_and_continue() {
        // 无 patches
        let empty = PatchInfo {
            loop_end: 0,
            break_patches: vec![],
            continue_patches: vec![],
            continue_target: 0,
        };
        assert!(!empty.has_patches());

        // 只有 break
        let only_break = PatchInfo {
            loop_end: 0,
            break_patches: vec![10],
            continue_patches: vec![],
            continue_target: 0,
        };
        assert!(only_break.has_patches());

        // 只有 continue
        let only_continue = PatchInfo {
            loop_end: 0,
            break_patches: vec![],
            continue_patches: vec![20],
            continue_target: 0,
        };
        assert!(only_continue.has_patches());

        // 两者都有
        let both = PatchInfo {
            loop_end: 0,
            break_patches: vec![10],
            continue_patches: vec![20],
            continue_target: 0,
        };
        assert!(both.has_patches());
    }

    #[test]
    fn test_total_patches_sums_break_and_continue() {
        let pi = PatchInfo {
            loop_end: 0,
            break_patches: vec![1, 2, 3],
            continue_patches: vec![4, 5],
            continue_target: 0,
        };
        assert_eq!(pi.total_patches(), 5);

        let pi_empty = PatchInfo {
            loop_end: 0,
            break_patches: vec![],
            continue_patches: vec![],
            continue_target: 0,
        };
        assert_eq!(pi_empty.total_patches(), 0);
    }
}
