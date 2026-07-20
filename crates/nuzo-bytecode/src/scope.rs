//! # 词法作用域管理系统
//!
//! ## 模块定位
//! 本模块实现 Nuzo 编译器的**词法作用域 (Lexical Scoping)** 机制，
//! 负责跟踪变量定义、解析变量引用、以及管理作用域生命周期。
//!
//! ## 核心组件
//!
//! ### 1. `Scope` — 本地作用域
//! 管理函数/块内的局部变量，支持：
//! - **块级嵌套**: 通过 `begin_scope()` / `end_scope()` 实现作用域压栈/出栈
//! - **变量遮蔽 (Shadowing)**: 内层变量可覆盖外层同名变量
//! - **最近优先解析**: 从内向外查找，返回最内层的匹配
//!
//! ### 2. `GlobalScope` — 全局作用域
//! 管理程序级别的全局变量，支持：
//! - **动态注册**: 运行时添加新全局变量
// - **更新语义**: 重定义已存在变量时更新值（不创建新条目）
// - **索引访问**: 通过 usize 索引快速读写（优于 HashMap 查找）
//!
//! ### 3. `ScopeKind` — 变量来源枚举
// 区分变量的存储位置，用于生成正确的字节码指令：
// - `Local(reg)`: 局部变量 → 使用寄存器直接访问
// - `Global(idx)`: 全局变量 → 使用 GetGlobal/SetGlobal 指令
//!
//! ## 设计原则
//!
//! ### 编译期 vs 运行期
//! 本模块的数据结构在**编译期**构建和销毁，
//! 不参与运行时的变量查找（那是由虚拟机的寄存器和全局表完成的）。
//!
//! ### 错误处理策略
//! - `resolve()` 返回 `Option<ScopeKind>`: 变量未定义时返回 None（编译错误）
//! - `define()` 允许重定义: 更新同一作用域的已有变量（用于多次赋值）
//! - `end_scope()` 安全退出: depth=0 时为空操作（防止欠还）
//!
//! ## 典型使用流程
//! ```ignore
//! // 1. 创建编译器上下文
//! let mut scope = Scope::new();
//! let mut globals = GlobalScope::new();
//!
//! // 2. 定义全局变量
//! globals.define("print", Value::from_builtin(print_fn));
//!
//! // 3. 进入函数作用域
//! scope.begin_scope();
//! scope.define("x", 0); // x 存储在寄存器 r0
//! scope.define("y", 1); // y 存储在寄存器 r1
//!
//! // 4. 解析变量引用
//! match scope.resolve_or_global("x", &globals) {
//!     Some(ScopeKind::Local(reg)) => {
//!         // 生成局部变量加载指令
//!         chunk.emit(Instruction::Mov { dest: Reg(2), src: Reg(reg) });
//!     }
//!     Some(ScopeKind::Global(idx)) => {
//!         // 生成全局变量加载指令
//!         chunk.emit(Instruction::GetGlobal { dest: Reg(2), name: ConstIdx(idx as u16) });
//!     }
//!     None => {
//!         // 编译错误：未定义的变量
//!         error!("Undefined variable: {}", name);
//!     }
//! }
//!
//! // 5. 退出作用域（自动清理局部变量）
//! scope.end_scope(); // x 和 y 在此之后不可见
//! ```

use nuzo_core::{XxHashMap, xx_hash_map_new};

use nuzo_core::Value;

/// 词法作用域 — 管理局部变量的定义与解析
///
/// # 数据结构
/// 内部使用 `Vec<ScopeVar>` 存储所有层级的变量，
/// 通过 `depth` 字段标记每个变量所属的作用域层级。
///
/// # 生命周期
/// ```text
/// Scope::new()           ← depth=0, 空作用域
///   ├─ define("x", 0)    ← 在 depth=0 定义 x
///   ├─ begin_scope()     ← depth=1, 进入内层块
///   │   ├─ define("y", 1)
///   │   └─ end_scope()   ← depth=0, y 被移除
///   └─ resolve("x")      ✓ 找到 (depth=0)
///     resolve("y")       ✗ 未找到
/// ```
///
/// # 变量遮蔽示例
/// ```ignore
/// scope.define("x", 0);  // 外层 x → r0
/// scope.begin_scope();
/// scope.define("x", 1);  // 内层 x → r1 (遮蔽外层)
/// assert_eq!(scope.resolve("x"), Some(ScopeKind::Local(1))); // 返回内层的 r1
/// scope.end_scope();
/// assert_eq!(scope.resolve("x"), Some(ScopeKind::Local(0))); // 恢复外层的 r0
/// ```
///
/// # 性能特征
/// - `define()`: O(1) 平摊 (Vec::push)
/// - `resolve()`: O(n) 最坏情况 (线性扫描，n 为变量总数)
/// - `end_scope()`: O(k) 反向 pop，k 为本层定义的变量数
///
/// 对于典型的编译场景（每个作用域 <100 个变量），性能完全足够。
pub struct Scope {
    /// 所有层级的变量列表（按定义顺序排列）
    vars: Vec<ScopeVar>,

    /// 当前作用域深度（0 = 全局/顶层）
    depth: usize,
}

/// 作用域变量的内部表示（私有类型）
struct ScopeVar {
    /// 变量名称
    name: String,

    /// 分配的虚拟机寄存器编号
    reg: u16,

    /// 定义所在的作用域深度
    depth: usize,
}

/// 全局作用域 — 管理程序级别的全局变量
///
/// # 设计目标
/// 提供一个编译期和运行时共享的全局变量注册表：
/// - **编译期**: 注册全局变量名并分配索引（用于 GetGlobal/SetGlobal 指令）
/// - **运行时**: 通过索引快速读写全局变量的值
///
/// # 数据结构选择
/// 使用 `HashMap<String, usize>` + `Vec<Value>` 的双索引设计：
/// - **名称→索引映射**: O(1) 按名查找（用于编译器解析）
/// - **索引→值数组**: O(1) 按索引访问（用于虚拟机执行）
///
/// 这种分离使得频繁的索引访问无需 Hash 计算，
/// 同时保持名称查找的灵活性。
///
/// # 语义特点
/// - **更新而非新增**: 重定义已有变量时更新值，返回原索引
/// - **动态扩展**: 可在程序运行时添加新全局变量
/// - **边界安全**: 越界访问时自动扩展数组（填充默认值）
///
/// # 使用场景
/// ```ignore
/// let mut globals = GlobalScope::new();
///
/// // 定义内置函数
/// let print_idx = globals.define("print", Value::from_builtin(print_fn));
///
/// // 定义用户全局变量
/// let pi_idx = globals.define("PI", Value::from_number(3.14159));
///
/// // 更新已有变量 (返回相同索引)
/// let pi_idx2 = globals.define("PI", Value::from_number(3.14));
/// assert_eq!(pi_idx, pi_idx2); // 索引不变，值已更新
///
/// // 通过索引读取 (O(1))
/// let val = globals.get(pi_idx).unwrap();
/// ```
pub struct GlobalScope {
    /// 变量名 → 值数组索引的映射
    names: XxHashMap<String, usize>,

    /// 按索引存储的全局变量值（稠密数组）
    values: Vec<Value>,
}

impl Default for Scope {
    fn default() -> Self {
        Self::new()
    }
}

impl Scope {
    /// 创建空的本地作用域 (depth=0)
    ///
    /// 初始状态无任何变量，depth 为 0（顶层作用域）。
    pub fn new() -> Self {
        Self { vars: Vec::new(), depth: 0 }
    }

    /// 获取当前作用域深度
    ///
    /// # 返回值
    /// - 0: 顶层/全局作用域
    /// - 1: 第一层嵌套块（如 if 体、循环体）
    /// - n: 第 n 层嵌套块
    pub fn depth(&self) -> usize {
        self.depth
    }

    /// 进入新的嵌套作用域 (depth += 1)
    ///
    /// 用于进入以下代码块：
    /// - 函数体
    /// - if/else 分支
    /// - while/for 循环体
    /// - 代码块 `{ ... }`
    ///
    /// # 示例
    /// ```ignore
    /// scope.begin_scope(); // depth: 0 → 1
    /// scope.define("tmp", 5); // tmp 仅在此作用域可见
    /// // ... 使用 tmp ...
    /// scope.end_scope(); // depth: 1 → 0, tmp 被移除
    /// ```
    pub fn begin_scope(&mut self) {
        self.depth += 1;
    }

    /// 退出当前作用域 (depth -= 1)，清理该层定义的所有变量
    ///
    /// # 安全保证
    /// - 当 depth=0 时为空操作（防止欠还错误）
    /// - 自动移除所有 depth > new_depth 的变量
    ///
    /// # 性能
    /// 利用 [`Scope::define`] 的不变量：**新变量总是 push 到 `vars` 末尾**，
    /// 而更新已有变量（同深度同名）只修改字段、不改变位置。
    /// 因此更深层作用域定义的变量在 `vars` 中位于末尾连续区域，
    /// 反向 pop 直到末尾变量 depth ≤ new_depth 即可，复杂度 O(k)（k 为本层
    /// 定义的变量数），优于原 `Vec::retain` 的 O(n)（n 为所有变量数）。
    pub fn end_scope(&mut self) {
        if self.depth == 0 {
            return;
        }
        self.depth -= 1;
        // 反向 pop：仅移除末尾 depth > self.depth 的变量。
        // 中间位置（被 update 的同深度变量）不受影响。
        while let Some(last) = self.vars.last() {
            if last.depth > self.depth {
                self.vars.pop();
            } else {
                break;
            }
        }
    }

    /// 在当前作用域定义（或更新）变量
    ///
    /// # 语义
    /// - 如果**同一深度**已有同名变量：更新其寄存器编号（不创建新条目）
    /// - 否则：创建新变量条目，绑定到当前 depth
    ///
    /// # 参数
    /// - `name`: 变量标识符（如 "x", "count", "temp"）
    /// - `reg`: 分配的虚拟机寄存器编号
    ///
    /// # 使用场景
    /// ```ignore
    /// scope.define("x", 0); // 首次定义 x → r0
    /// scope.define("x", 2); // 更新 x → r2 (同一 depth)
    /// ```
    pub fn define(&mut self, name: &str, reg: u16) {
        if let Some(existing) =
            self.vars.iter_mut().rev().find(|v| v.name == name && v.depth == self.depth)
        {
            existing.reg = reg;
        } else {
            self.vars.push(ScopeVar { name: name.to_string(), reg, depth: self.depth });
        }
    }

    /// 解析局部变量（仅搜索本地作用域，不含全局）
    ///
    /// # 查找策略
    /// 从**最近定义**的变量开始向前搜索（反向迭代），
    /// 返回第一个匹配的变量（实现变量遮蔽语义）。
    ///
    /// # 返回值
    /// - `Some(ScopeKind::Local(reg))`: 找到变量，返回寄存器编号
    /// - `None`: 未找到（可能需要查询全局作用域或报错）
    ///
    /// # 示例
    /// ```ignore
    /// scope.define("x", 0); // 外层 x → r0
    /// scope.begin_scope();
    /// scope.define("x", 1); // 内层 x → r1 (遮蔽外层)
    /// assert_eq!(scope.resolve("x"), Some(ScopeKind::Local(1))); // 返回内层的 r1
    /// ```
    pub fn resolve(&self, name: &str) -> Option<ScopeKind> {
        for v in self.vars.iter().rev() {
            if v.name == name {
                return Some(ScopeKind::Local(v.reg));
            }
        }
        None
    }

    /// 解析变量（先查局部，再查全局）
    ///
    /// 这是编译器解析变量引用的主要入口点，
    /// 实现了 Nuzo 的**局部优先 + 全局回退**查找策略。
    ///
    /// # 参数
    /// - `name`: 要解析的变量名
    /// - `globals`: 全局作用域引用（用于回退查找）
    ///
    /// # 返回值
    /// - `Some(ScopeKind::Local(reg))`: 局部变量
    /// - `Some(ScopeKind::Global(idx))`: 全局变量
    /// - `None`: 变量未定义（编译错误）
    ///
    /// # 典型用法
    /// ```ignore
    /// match scope.resolve_or_global("myVar", &globals) {
    ///     Some(ScopeKind::Local(reg)) => {
    ///         // 生成: Mov dest, reg
    ///     }
    ///     Some(ScopeKind::Global(idx)) => {
    ///         // 生成: GetGlobal dest, ConstIdx(idx)
    ///     }
    ///     None => {
    ///         compiler_error!("Undefined variable: {}", name);
    ///     }
    /// }
    /// ```
    pub fn resolve_or_global(&self, name: &str, globals: &GlobalScope) -> Option<ScopeKind> {
        if let Some(kind) = self.resolve(name) {
            return Some(kind);
        }
        globals.resolve(name).map(ScopeKind::Global)
    }

    /// 获取指定深度的所有变量名（用于调试和 IDE 支持）
    ///
    /// # 参数
    /// - `depth`: 目标作用域深度
    ///
    /// # 返回值
    /// 该深度的变量名称列表（按定义顺序）
    ///
    /// # 使用场景
    /// - IDE 自动补全：显示当前可见的变量列表
    /// - 调试输出：打印作用域内容
    /// - 单元测试：验证作用域状态
    pub fn locals_at_depth(&self, depth: usize) -> Vec<&str> {
        self.vars.iter().filter(|v| v.depth == depth).map(|v| v.name.as_str()).collect()
    }

    /// 获取当前活跃的所有局部变量的寄存器编号
    ///
    /// "活跃"指 depth <= 当前深度的变量（即未被 end_scope 清理的变量）。
    ///
    /// # 返回值
    /// 寄存器编号列表（可能有重复，如果多个别名指向同一寄存器）
    ///
    /// # 使用场景
    /// - 计算函数所需的寄存器窗口大小
    /// - 生成调试符号信息
    pub fn active_locals(&self) -> Vec<u16> {
        self.vars.iter().filter(|v| v.depth <= self.depth).map(|v| v.reg).collect()
    }

    /// 根据寄存器编号反向查找变量名（用于调试输出）
    ///
    /// # 参数
    /// - `reg`: 虚拟机寄存器编号
    ///
    /// # 返回值
    /// - `Some(name)`: 找到该寄存器对应的最新变量名
    /// - `None`: 该寄存器未绑定任何活跃变量
    ///
    /// # 注意
    /// 如果多个变量绑定到同一寄存器（寄存器重用），返回最近定义的那个。
    pub fn find_name_by_reg(&self, reg: u16) -> Option<String> {
        for v in self.vars.iter().rev() {
            if v.reg == reg && v.depth <= self.depth {
                return Some(v.name.clone());
            }
        }
        None
    }

    /// 获取所有变量名（跨所有深度，用于完整状态导出）
    pub fn all_names(&self) -> Vec<String> {
        self.vars.iter().map(|v| v.name.clone()).collect()
    }

    /// 获取所有局部变量及其寄存器绑定（跨所有深度）
    ///
    /// # 返回值
    /// `(变量名, 寄存器编号)` 元组列表
    pub fn all_locals(&self) -> Vec<(String, u16)> {
        self.vars.iter().map(|v| (v.name.clone(), v.reg)).collect()
    }

    /// 重新绑定已有变量到新寄存器（跨深度搜索）
    ///
    /// 与 `define()` 不同，此方法**不限制深度**，
    /// 会更新最近的同名变量（无论其定义在哪一层）。
    ///
    /// # 参数
    /// - `name`: 目标变量名
    /// - `new_reg`: 新的寄存器编号
    ///
    /// # 返回值
    /// - `true`: 找到并更新了变量
    /// - `false`: 变量不存在
    ///
    /// # 使用场景
    /// 寄存器分配器优化后需要更新变量绑定。
    pub fn rebind(&mut self, name: &str, new_reg: u16) -> bool {
        if let Some(var) = self.vars.iter_mut().rev().find(|v| v.name == name) {
            var.reg = new_reg;
            true
        } else {
            false
        }
    }
}

/// 变量来源类型 — 区分局部变量和全局变量
///
/// # 设计目的
/// 编译器在解析变量引用后需要生成不同的字节码指令：
/// - **Local**: 变量存储在虚拟机寄存器中，使用 `Mov` 等指令访问
/// - **Global**: 变量存储在全局表中，使用 `GetGlobal`/`SetGlobal` 指令访问
///
/// 通过此枚举，编译器可以使用统一的 match 语句处理两种情况，
/// 而无需在调用点区分变量来源。
///
/// # 使用示例
/// ```ignore
/// fn compile_variable_access(scope: &Scope, globals: &GlobalScope, name: &str) {
///     match scope.resolve_or_global(name, globals) {
///         Some(ScopeKind::Local(reg)) => {
///             // 局部变量：直接使用寄存器
///             emit(Instruction::Mov { dest: dest_reg, src: Reg(reg) });
///         }
///         Some(ScopeKind::Global(idx)) => {
///             // 全局变量：通过名称索引访问
///             let name_const = chunk.add_constant(Value::from_string(name));
///             emit(Instruction::GetGlobal { dest: dest_reg, name: ConstIdx(name_const as u16) });
///         }
///         None => {
///             // 编译错误：未定义的变量
///             error!("Undefined variable '{}'", name);
///         }
///     }
/// }
/// ```
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum ScopeKind {
    /// 局部变量 — 存储在虚拟机寄存器中
    ///
    /// 参数 `reg` 是寄存器编号 (0..=65535)，
    /// 对应 `Reg` 操作数类型。
    Local(u16),

    /// 全局变量 — 存储在全局变量表中
    ///
    /// 参数 `idx` 是全局变量数组中的索引位置 (0..=N)，
    /// 用于 `GetGlobal`/`SetGlobal` 指令的常量池索引。
    Global(usize),
}

impl Default for GlobalScope {
    fn default() -> Self {
        Self::new()
    }
}

impl GlobalScope {
    /// 创建空的全局作用域（无预定义变量）
    ///
    /// 初始状态：
    /// - `names`: 空 HashMap
    /// - `values`: 空 Vec
    pub fn new() -> Self {
        Self { names: xx_hash_map_new(), values: Vec::new() }
    }

    /// 定义（或更新）全局变量
    ///
    /// # 语义
    /// - 如果变量**已存在**: 更新其值，返回原索引（不创建新条目）
    /// - 如果变量**不存在**: 追加到 values 数组末尾，返回新索引
    ///
    /// 这种"更新语义"确保全局变量的索引稳定，
    /// 即使多次赋值也不会改变其在表中的位置。
    ///
    /// # 参数
    /// - `name`: 全局变量名（如 "print", "PI", "VERSION"）
    /// - `value`: 变量的初始值（可以是任意 Value 类型）
    ///
    /// # 返回值
    /// 变量在 `values` 数组中的索引位置 (usize)
    ///
    /// # 示例
    /// ```ignore
    /// let idx1 = globals.define("x", Value::from_number(1.0)); // idx1 = 0
    /// let idx2 = globals.define("x", Value::from_number(2.0)); // idx2 = 0 (更新)
    /// assert_eq!(idx1, idx2); // 索引不变
    /// assert_eq!(globals.get(0), Some(Value::from_number(2.0))); // 值已更新
    /// ```
    pub fn define(&mut self, name: &str, value: Value) -> usize {
        if let Some(&idx) = self.names.get(name) {
            self.values[idx] = value;
            idx
        } else {
            let idx = self.values.len();
            self.values.push(value);
            self.names.insert(name.to_string(), idx);
            idx
        }
    }

    /// 按名称查找全局变量索引
    ///
    /// # 参数
    /// - `name`: 要查找的变量名
    ///
    /// # 返回值
    /// - `Some(idx)`: 找到变量，返回其在 values 数组中的索引
    /// - `None`: 变量不存在
    ///
    /// # 性能
    /// O(1) 平均时间复杂度（HashMap 查找）。
    pub fn resolve(&self, name: &str) -> Option<usize> {
        self.names.get(name).copied()
    }

    /// 按索引读取全局变量值
    ///
    /// # 参数
    /// - `idx`: 全局变量索引 (0..=len-1)
    ///
    /// # 返回值
    /// - `Some(Value)`: 索引有效时返回值的克隆
    /// - `None`: 索引越界时返回 None
    ///
    /// # 安全性
    /// 总是返回克隆的 Value，防止外部修改内部状态。
    pub fn get(&self, idx: usize) -> Option<Value> {
        self.values.get(idx).copied()
    }

    /// 按索引设置全局变量值（支持自动扩展）
    ///
    /// # 参数
    /// - `idx`: 目标索引
    /// - `value`: 新值
    ///
    /// # 行为
    /// - **索引有效**: 直接更新 `values[idx]`
    /// - **索引越界**: 自动扩展数组，中间填充 `Value::default()` (nil)
    ///
    /// # 使用场景
    /// 虚拟机的 `SetGlobal` 指令执行时会调用此方法。
    pub fn set(&mut self, idx: usize, value: Value) {
        if idx < self.values.len() {
            self.values[idx] = value;
        } else {
            self.values.resize(idx, Value::default());
            self.values.push(value);
        }
    }

    /// 获取全局变量总数
    pub fn len(&self) -> usize {
        self.values.len()
    }

    /// 检查是否无任何全局变量
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    /// 获取所有全局变量名称
    ///
    /// # 返回值
    /// 所有已注册全局变量名的列表（无特定顺序，取决于 HashMap 内部迭代顺序）
    ///
    /// # 性能
    /// O(n) 其中 n 为全局变量数量
    ///
    /// # 使用场景
    /// - 运行时调试器：列出所有全局变量
    /// - REPL 自动补全：提供全局变量名候选
    /// - 反射 API：查询全局命名空间
    pub fn names(&self) -> Vec<String> {
        self.names.keys().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nuzo_values::{FALSE, NIL, TRUE};
    use std::assert_matches;

    #[test]
    fn test_scope_define_and_resolve() {
        let mut scope = Scope::new();
        scope.define("x", 0);
        scope.define("y", 1);

        assert_matches!(scope.resolve("x"), Some(ScopeKind::Local(0)));
        assert_matches!(scope.resolve("y"), Some(ScopeKind::Local(1)));
        assert!(scope.resolve("z").is_none());
    }

    #[test]
    fn test_scope_block_isolation() {
        let mut scope = Scope::new();
        scope.define("x", 0);

        scope.begin_scope();
        scope.define("y", 1);
        assert_matches!(scope.resolve("x"), Some(ScopeKind::Local(0)));
        assert_matches!(scope.resolve("y"), Some(ScopeKind::Local(1)));

        scope.end_scope();
        assert_matches!(scope.resolve("x"), Some(ScopeKind::Local(0)));
        assert!(scope.resolve("y").is_none());
    }

    #[test]
    fn test_scope_shadowing() {
        let mut scope = Scope::new();
        scope.define("x", 0);

        scope.begin_scope();
        scope.define("x", 1);
        assert_matches!(scope.resolve("x"), Some(ScopeKind::Local(1)));

        scope.end_scope();
        assert_matches!(scope.resolve("x"), Some(ScopeKind::Local(0)));
    }

    #[test]
    fn test_scope_nested_blocks() {
        let mut scope = Scope::new();
        scope.define("a", 0);

        scope.begin_scope();
        scope.define("b", 1);

        scope.begin_scope();
        scope.define("c", 2);
        assert_matches!(scope.resolve("a"), Some(ScopeKind::Local(0)));
        assert_matches!(scope.resolve("b"), Some(ScopeKind::Local(1)));
        assert_matches!(scope.resolve("c"), Some(ScopeKind::Local(2)));

        scope.end_scope();
        assert!(scope.resolve("c").is_none());
        assert_matches!(scope.resolve("b"), Some(ScopeKind::Local(1)));

        scope.end_scope();
        assert!(scope.resolve("b").is_none());
        assert_matches!(scope.resolve("a"), Some(ScopeKind::Local(0)));
    }

    #[test]
    fn test_global_scope() {
        let mut globals = GlobalScope::new();
        let idx = globals.define("pi", Value::from_number(2.5));

        assert_eq!(globals.resolve("pi"), Some(idx));
        assert_eq!(globals.get(idx).unwrap().as_number(), 2.5);
        assert!(globals.resolve("unknown").is_none());
    }

    #[test]
    fn test_global_scope_update() {
        let mut globals = GlobalScope::new();
        let idx1 = globals.define("x", Value::from_number(1.0));
        let idx2 = globals.define("x", Value::from_number(2.0));

        assert_eq!(idx1, idx2);
        assert_eq!(globals.get(idx1).unwrap().as_number(), 2.0);
    }

    #[test]
    fn test_resolve_or_global_fallback() {
        let mut scope = Scope::new();
        let mut globals = GlobalScope::new();

        scope.define("local_var", 0);
        globals.define("global_var", Value::from_number(42.0));

        assert_matches!(scope.resolve_or_global("local_var", &globals), Some(ScopeKind::Local(0)));

        if let Some(ScopeKind::Global(idx)) = scope.resolve_or_global("global_var", &globals) {
            assert_eq!(globals.get(idx).unwrap().as_number(), 42.0);
        } else {
            panic!("Expected Global scope kind");
        }

        assert!(scope.resolve_or_global("unknown", &globals).is_none());
    }

    #[test]
    fn test_scope_end_at_zero_is_noop() {
        let mut scope = Scope::new();
        scope.define("x", 0);
        scope.end_scope();
        assert_matches!(scope.resolve("x"), Some(ScopeKind::Local(0)));
    }

    #[test]
    fn test_global_scope_set() {
        let mut globals = GlobalScope::new();
        let idx = globals.define("flag", TRUE);
        globals.set(idx, FALSE);
        assert_eq!(globals.get(idx).unwrap(), FALSE);
    }

    #[test]
    fn test_global_scope_len() {
        let mut globals = GlobalScope::new();
        assert_eq!(globals.len(), 0);
        globals.define("a", NIL);
        globals.define("b", TRUE);
        assert_eq!(globals.len(), 2);
    }
}
