//! IR 核心数据类型 — 目标无关的三地址码中间表示

use std::path::PathBuf;
use std::sync::Arc;

// ============================================================================
// ID 类型 — Newtype 包装器，编译期防止混用
// ============================================================================

/// IR 值引用 — SSA 风格的虚拟寄存器引用
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ValueRef(pub u32);

impl ValueRef {
    pub const MAX: u32 = u32::MAX;
    pub fn new(id: u32) -> Self {
        ValueRef(id)
    }
}

/// 基本块 ID（控制流图节点）
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct BasicBlockId(pub u32);

impl BasicBlockId {
    pub const INVALID: Self = BasicBlockId(u32::MAX);
    pub fn new(id: u32) -> Self {
        BasicBlockId(id)
    }
}

/// 函数 ID（IR 层面的函数引用）
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct IrFunctionId(pub u32);

impl IrFunctionId {
    pub fn new(id: u32) -> Self {
        IrFunctionId(id)
    }
}

// ============================================================================
// 常量与运算符
// ============================================================================

/// IR 常量 — 字面量值（目标无关，不依赖 nuzo_core::Value）
#[derive(Clone, Debug, PartialEq)]
pub enum IrConstant {
    Number(f64),
    String(Arc<str>),
    Bool(bool),
    Nil,
}

/// IR 二元运算符
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum IrBinOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Pow,
    Eq,
    Neq,
    Lt,
    Gt,
    Le,
    Ge,
}

/// 从 AST BinaryOp 到 IR IrBinOp 的单一映射源（Single Source of Truth）。
///
/// 此 `From` 实现是 `ast::BinaryOp → IrBinOp` 的唯一映射定义。
/// `builder.rs::build_binary` 和其他需要转换的代码都应使用 `.into()` 或 `IrBinOp::from(op)`。
///
/// # 编译期完整性保证
/// 如果 `BinaryOp` 新增变体而忘记在此映射中添加对应分支，
/// `match` 将非 exhaustive，编译器直接报错。
impl From<nuzo_frontend::ast::BinaryOp> for IrBinOp {
    #[inline(always)]
    fn from(op: nuzo_frontend::ast::BinaryOp) -> Self {
        match op {
            nuzo_frontend::ast::BinaryOp::Add => IrBinOp::Add,
            nuzo_frontend::ast::BinaryOp::Sub => IrBinOp::Sub,
            nuzo_frontend::ast::BinaryOp::Mul => IrBinOp::Mul,
            nuzo_frontend::ast::BinaryOp::Div => IrBinOp::Div,
            nuzo_frontend::ast::BinaryOp::Mod => IrBinOp::Mod,
            nuzo_frontend::ast::BinaryOp::Pow => IrBinOp::Pow,
            nuzo_frontend::ast::BinaryOp::Eq => IrBinOp::Eq,
            nuzo_frontend::ast::BinaryOp::Neq => IrBinOp::Neq,
            nuzo_frontend::ast::BinaryOp::Lt => IrBinOp::Lt,
            nuzo_frontend::ast::BinaryOp::Gt => IrBinOp::Gt,
            nuzo_frontend::ast::BinaryOp::LtEq => IrBinOp::Le,
            nuzo_frontend::ast::BinaryOp::GtEq => IrBinOp::Ge,
        }
    }
}

impl IrBinOp {
    /// 运算符的文本表示（用于 display）
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Add => "+",
            Self::Sub => "-",
            Self::Mul => "*",
            Self::Div => "/",
            Self::Mod => "%",
            Self::Pow => "**",
            Self::Eq => "==",
            Self::Neq => "!=",
            Self::Lt => "<",
            Self::Gt => ">",
            Self::Le => "<=",
            Self::Ge => ">=",
        }
    }
}

/// IR 一元运算符
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum IrUnaryOp {
    Neg, // 算术取负
    Not, // 逻辑非
}

/// 从 AST UnaryOp 到 IR IrUnaryOp 的单一映射源（Single Source of Truth）。
///
/// 与 `From<ast::BinaryOp> for IrBinOp` 对称，建立完整的运算符映射 SSOT。
impl From<nuzo_frontend::ast::UnaryOp> for IrUnaryOp {
    #[inline(always)]
    fn from(op: nuzo_frontend::ast::UnaryOp) -> Self {
        match op {
            nuzo_frontend::ast::UnaryOp::Negate => IrUnaryOp::Neg,
            nuzo_frontend::ast::UnaryOp::Not => IrUnaryOp::Not,
        }
    }
}

impl IrUnaryOp {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Neg => "-",
            Self::Not => "!",
        }
    }
}

// ============================================================================
// IR 操作码 — 目标无关的三地址指令集
// ============================================================================

/// IR 操作码枚举
///
/// 设计原则：
/// - 三地址形式：每条指令最多 1 个目标 + 2 个源操作数
/// - 目标无关：不包含寄存器编号、常量池索引等后端概念
/// - 基本块引用：控制流通过 BasicBlockId 连接
#[derive(Clone, Debug, PartialEq)]
pub enum IrOp {
    // ── 字面量加载 ──
    LoadConstant { dest: ValueRef, constant: IrConstant },
    LoadArg { dest: ValueRef, index: u16 },

    // ── 二元运算 ──
    Binary { dest: ValueRef, op: IrBinOp, left: ValueRef, right: ValueRef },

    // ── 一元运算 ──
    Unary { dest: ValueRef, op: IrUnaryOp, operand: ValueRef },

    // ── 寄存器移动（用于控制流值合并，degenerate Phi）──
    // 当 if/and/or 等控制流需要在多个分支中写入同一结果值时，
    // 通过 Mov 将各分支的值移动到统一的 result ValueRef。
    // 语义上等价于 Phi 节点，但实现更简单（无需 predecessor 追踪）。
    Mov { dest: ValueRef, src: ValueRef },

    // ── 函数操作 ──
    Call { dest: Option<ValueRef>, callee: ValueRef, args: Vec<ValueRef> },
    Closure { dest: ValueRef, ir_func: IrFunctionId },
    // 闭包捕获变量填充：在父函数中发射，将值写入闭包的 captured[] 数组
    // 对应字节码 Instruction::Capture
    Capture { closure: ValueRef, index: u16, source: CaptureSource },

    // ── 变量访问 ──
    GetLocal { dest: ValueRef, name: Arc<str> },
    SetLocal { name: Arc<str>, value: ValueRef },
    GetGlobal { dest: ValueRef, name: Arc<str> },
    SetGlobal { name: Arc<str>, value: ValueRef },
    GetCapture { dest: ValueRef, index: u16 },
    SetCapture { index: u16, value: ValueRef },

    // ── 控制流 ──
    Jump { target: BasicBlockId },
    JumpIf { cond: ValueRef, then_target: BasicBlockId, else_target: BasicBlockId },
    Return { value: Option<ValueRef> },

    // ── 复合类型 ──
    ArrayNew { dest: ValueRef, elements: Vec<ValueRef> },
    ObjectNew { dest: ValueRef },
    GetField { dest: ValueRef, object: ValueRef, field: Arc<str> },
    SetField { object: ValueRef, field: Arc<str>, value: ValueRef },
    IndexGet { dest: ValueRef, object: ValueRef, index: ValueRef },
    IndexSet { object: ValueRef, index: ValueRef, value: ValueRef },
    // 原地修改数组元素（简单标识符引用，引用语义，零克隆）
    // 对应字节码 Instruction::SetIndexMut
    IndexSetMut { object: ValueRef, index: ValueRef, value: ValueRef },

    // ── 条件选择（Phi 节点的显式形式）──
    // dest = condition ? then_value : else_value
    // 用于跨基本块的变量重赋值场景：if 分支中变量被重新赋值，
    // 在 merge 点需要根据运行时走哪条路来选择正确的值。
    // codegen 翻译为: if !condition jmp L_else; mov dest, then_value; jmp L_end; L_else: mov dest, else_value; L_end:
    Select { condition: ValueRef, then_value: ValueRef, else_value: ValueRef, dest: ValueRef },

    // ── 长度查询 ──
    // 获取对象长度（数组元素数、字符串字符数等）
    // 对应字节码 Instruction::Len
    Len { dest: ValueRef, object: ValueRef },

    // ── 范围构造 ──
    // 创建范围对象 dest = start..end (inclusive 控制闭/半开区间)
    // 对应字节码 Instruction::RangeNew
    RangeNew { dest: ValueRef, start: ValueRef, end: ValueRef, inclusive: bool },

    // ── 字符串批量拼接 ──
    // 编译期将连续 `+` 链展平为操作数列表，生成单条 StringBuild 指令。
    // VM 一次性计算总长度、分配缓冲区、逐段拷入，避免 O(N²) 中间分配。
    // 对应字节码 Instruction::StringBuild
    //
    // # 设计决策
    // 该指令在 IR 构建阶段（非优化阶段）生成，因为需要利用 AST 拓扑信息
    // 识别连续 `+` 链。优化器可能破坏 AST 拓扑（如常量折叠后操作数合并），
    // 因此在 IR 构建时尽早捕获。
    //
    // # 语义
    // `dest = concat(operands[0], operands[1], ..., operands[N-1])`
    // 当 operands.len() == 1 时退化为 Mov（codegen 特殊处理）。
    // 当 operands.len() == 0 时产生空字符串。
    StringBuild { dest: ValueRef, operands: Vec<ValueRef> },

    // ── 切片链字符串构建器 (SCSB) ──
    // 零拷贝循环内字符串拼接：编译器检测 `s = s + expr` 模式后生成。
    // 循环外创建 SliceChain，循环内 append，循环后 finish。
    // 对应字节码 Instruction::SliceChainNew / SliceChainAppend / SliceChainFinish
    //
    // SliceChainInit: 在循环前创建空 SliceChain，存储到临时变量
    SliceChainInit { dest: ValueRef },
    // SliceChainAppend: 循环体内追加字符串（O(1) 引用计数）
    SliceChainAppend { chain: ValueRef, src: ValueRef },
    // SliceChainFinish: 循环后完成拼接，返回结果字符串
    SliceChainFinish { dest: ValueRef, chain: ValueRef },

    // ── 异常处理 ──
    // TryStart 标记 try 块开始，catch_target 指向 catch 块的基本块 ID。
    // exception_reg 是 VM 用于接收异常值的寄存器编号（u8，与字节码 TryStart 一致）。
    // codegen 时将 catch_target 转换为相对偏移 catch_offset。
    TryStart { catch_target: BasicBlockId, exception_reg: u8 },
    // TryEnd 标记 try 块正常结束（无异常抛出时的清理点）。
    TryEnd,
    // Out 抛出异常：取 value 的值，查找最近的 TryStart，跳转到对应 catch 块。
    // 语义上等价于终止指令（抛出后控制流不再顺序执行）。
    Out { value: ValueRef },

    // ── 打印 / 调试 ──
    Print { value: ValueRef },
}

impl IrOp {
    /// 判断是否是终止指令（基本块的最后一条指令必须是终止指令）
    pub fn is_terminator(&self) -> bool {
        matches!(
            self,
            Self::Jump { .. } | Self::JumpIf { .. } | Self::Return { .. } | Self::Out { .. }
        )
    }

    /// 获取指令的目标 ValueRef（如果有）
    pub fn dest(&self) -> Option<ValueRef> {
        match self {
            Self::LoadConstant { dest, .. } => Some(*dest),
            Self::LoadArg { dest, .. } => Some(*dest),
            Self::Binary { dest, .. } => Some(*dest),
            Self::Unary { dest, .. } => Some(*dest),
            Self::Mov { dest, .. } => Some(*dest),
            Self::Call { dest, .. } => *dest,
            Self::Closure { dest, .. } => Some(*dest),
            Self::GetLocal { dest, .. } => Some(*dest),
            Self::GetGlobal { dest, .. } => Some(*dest),
            Self::GetCapture { dest, .. } => Some(*dest),
            Self::ArrayNew { dest, .. } => Some(*dest),
            Self::ObjectNew { dest, .. } => Some(*dest),
            Self::GetField { dest, .. } => Some(*dest),
            Self::IndexGet { dest, .. } => Some(*dest),
            Self::Len { dest, .. } => Some(*dest),
            Self::RangeNew { dest, .. } => Some(*dest),
            Self::StringBuild { dest, .. } => Some(*dest),
            Self::SliceChainInit { dest } => Some(*dest),
            Self::SliceChainFinish { dest, .. } => Some(*dest),
            _ => None,
        }
    }

    /// 获取本指令作为源操作数引用的所有 ValueRef
    ///
    /// 返回指令读取（非写入）的所有值引用，用于数据流分析、
    /// 死代码消除、寄存器分配等优化 pass。
    pub fn src_value_refs(&self) -> Vec<ValueRef> {
        match self {
            // ── 二元运算：2 个源 ──
            Self::Binary { left, right, .. } => vec![*left, *right],

            // ── 一元运算：1 个源 ──
            Self::Unary { operand, .. } => vec![*operand],

            // ── 寄存器移动 ──
            Self::Mov { src, .. } => vec![*src],

            // ── 函数调用：callee + args ──
            Self::Call { callee, args, .. } => {
                std::iter::once(*callee).chain(args.iter().copied()).collect()
            }

            // ── 条件跳转 ──
            Self::JumpIf { cond, .. } => vec![*cond],

            // ── 返回 ──
            Self::Return { value } => value.map_or(vec![], |v| vec![v]),

            // ── 数组构造 ──
            Self::ArrayNew { elements, .. } => elements.to_vec(),

            // ── 字符串批量拼接 ──
            Self::StringBuild { operands, .. } => operands.to_vec(),

            // ── 切片链字符串构建器 (SCSB) ──
            Self::SliceChainInit { .. } => vec![],
            Self::SliceChainAppend { chain, src } => vec![*chain, *src],
            Self::SliceChainFinish { chain, .. } => vec![*chain],

            // ── 索引操作 ──
            Self::IndexGet { object, index, .. } => vec![*object, *index],
            Self::IndexSet { object, index, value } => vec![*object, *index, *value],
            Self::IndexSetMut { object, index, value } => vec![*object, *index, *value],

            // ── 字段操作 ──
            Self::GetField { object, .. } => vec![*object],
            Self::SetField { object, value, .. } => vec![*object, *value],

            // ── 条件选择（Phi 的显式形式）──
            Self::Select { condition, then_value, else_value, .. } => {
                vec![*condition, *then_value, *else_value]
            }

            // ── 长度查询 ──
            Self::Len { object, .. } => vec![*object],

            // ── 范围构造 ──
            Self::RangeNew { start, end, .. } => vec![*start, *end],

            // ── 异常抛出 ──
            Self::Out { value } => vec![*value],

            // ── 打印 ──
            Self::Print { value } => vec![*value],

            // ── 闭包捕获 ──
            Self::Capture { closure, source, .. } => match source {
                CaptureSource::Register(r) => vec![*closure, *r],
                CaptureSource::OuterCapture(_) => vec![*closure],
                CaptureSource::Global(_) => vec![*closure],
            },

            // ── 变量写入（源是 value）──
            Self::SetLocal { value, .. } => vec![*value],
            Self::SetGlobal { value, .. } => vec![*value],
            Self::SetCapture { value, .. } => vec![*value],

            // ── 无源操作数的变体 ──
            // LoadConstant, LoadArg, Closure, GetLocal, GetGlobal,
            // GetCapture, ObjectNew, Jump, TryStart, TryEnd
            _ => vec![],
        }
    }
}

// ============================================================================
// 基本块与函数
// ============================================================================

/// 基本块 — 包含线性指令序列
///
/// 基本块是控制流图的基本单元，具有以下性质：
/// - 入口：从一条 Jump/JumpIf/或函数入口进入
/// - 出口：最后一条指令必须是终止指令（Jump/JumpIf/Return）
/// - 线性：内部无分支（除了可能最后一条的条件跳转）
#[derive(Clone, Debug, Default)]
pub struct BasicBlock {
    pub id: BasicBlockId,
    pub instructions: Vec<IrOp>,
}

impl BasicBlock {
    pub fn new(id: BasicBlockId) -> Self {
        Self { id, instructions: Vec::new() }
    }

    /// 添加指令到基本块
    pub fn push(&mut self, op: IrOp) {
        self.instructions.push(op);
    }

    /// 判断基本块是否合法（有终止指令或为空）
    pub fn is_valid(&self) -> bool {
        self.instructions.is_empty()
            || self.instructions.last().is_some_and(|op| op.is_terminator())
    }
}

/// 闭包捕获描述
#[derive(Clone, Debug, PartialEq)]
pub struct CaptureDesc {
    pub name: Arc<str>,
    pub is_mutable: bool,
}

/// 捕获变量来源（用于 IrOp::Capture）
///
/// 对应 VM 字节码中 Capture 指令的 source 操作数：
/// - Register：从当前函数的寄存器直接捕获（变量在父函数的 locals 中）
/// - OuterCapture：从当前闭包的父级捕获列表中获取（跨层捕获）
#[derive(Clone, Debug, PartialEq)]
pub enum CaptureSource {
    /// 从父函数的寄存器捕获（source & CAPTURE_OUTER_FLAG == 0）
    Register(ValueRef),
    /// 从父闭包的 captured[] 中捕获（source | CAPTURE_OUTER_FLAG）
    OuterCapture(u16),
    /// 从全局变量捕获（codegen 会先发射 GetGlobal 加载到寄存器，再按值捕获）
    Global(Arc<str>),
}

/// IR 函数 — 由基本块组成的代码单元
///
/// Phase 1 设计：不建完整 CFG，仅线性排列基本块。
/// 每个 if/while/for 会产生新的 BasicBlock 分割。
#[derive(Clone, Debug)]
pub struct IrFunction {
    pub id: IrFunctionId,
    pub name: Arc<str>,
    pub params: Vec<Arc<str>>,
    pub blocks: Vec<BasicBlock>,
    pub entry_block: BasicBlockId,
    pub locals: Vec<Arc<str>>,
    pub captures: Vec<CaptureDesc>,
}

impl IrFunction {
    pub fn new(id: IrFunctionId, name: impl Into<Arc<str>>) -> Self {
        let name = name.into();
        let entry = BasicBlockId(0);
        Self {
            id,
            name,
            params: Vec::new(),
            blocks: vec![BasicBlock::new(entry)],
            entry_block: entry,
            locals: Vec::new(),
            captures: Vec::new(),
        }
    }

    /// 获取当前（最后一个）基本块的可变引用
    ///
    /// # 不变量
    /// `IrFunction::new()` 始终创建 entry block，因此 `self.blocks` 永远非空。
    /// `debug_assert!` 在 debug 构建中验证此不变量，release 构建中 `expect`
    /// 提供清晰的 panic 消息（仅在不变量被破坏时触发）。
    pub fn current_block_mut(&mut self) -> &mut BasicBlock {
        debug_assert!(!self.blocks.is_empty(), "IrFunction invariant violated: blocks is empty");
        self.blocks.last_mut().expect("at least entry block")
    }
}

// ============================================================================
// IR 模块 — 完整程序的 IR 表示
// ============================================================================

/// IR 模块 — 完整 Nuzo 程序的中间表示
///
/// 一个 Module 对应一个源文件/编译单元，
/// 包含所有顶层函数定义和全局变量声明。
#[derive(Clone, Debug, Default)]
pub struct IrModule {
    pub functions: Vec<IrFunction>,
    pub globals: Vec<Arc<str>>,
    pub constants: Vec<IrConstant>,
    /// 当前模块的源文件路径
    pub path: Option<PathBuf>,
    /// 当前模块的 import 记录
    pub imports: Vec<ImportRecord>,
}

/// Import 记录 — 描述一个 import 语句的解析结果
#[derive(Debug, Clone)]
pub struct ImportRecord {
    /// 已解析的绝对路径
    pub path: PathBuf,
    /// 是否为 lazy import
    pub lazy: bool,
    /// 该 import 引入的符号名列表（不含子模块 main 入口）
    ///
    /// `pre_scan_global_fns` 据此做重名检测并加入 `known_global_fns`，
    /// 使导入函数可被前向引用。
    pub resolved_symbols: Vec<String>,
    /// 模块别名（`as` 语法）
    pub alias: Option<String>,
    /// 被导入模块的全部 IrFunction 定义（含 main 入口，合并时按需过滤）
    ///
    /// `build_with_imports` 调用 `merge_imported_functions` 将这些函数定义
    /// 合并到主模块的 `functions` 列表，使 codegen 能自然编译导入的函数。
    /// 合并时会对 IrFunctionId 重新编号并重映射内部 `IrOp::Closure { ir_func }` 引用。
    pub functions: Vec<IrFunction>,
}

impl IrModule {
    pub fn new() -> Self {
        Self::default()
    }

    /// 创建新的 IrFunction 并加入模块，返回其 ID
    pub fn add_function(&mut self, name: impl Into<Arc<str>>) -> IrFunctionId {
        let id = IrFunctionId(self.functions.len() as u32);
        let func = IrFunction::new(id, name);
        self.functions.push(func);
        id
    }

    /// 获取当前正在构建的函数的可变引用
    ///
    /// # 不变量
    /// `IrBuilder::new()` 始终创建 main 函数，因此 `self.functions` 永远非空。
    /// `debug_assert!` 在 debug 构建中验证此不变量，release 构建中 `expect`
    /// 提供清晰的 panic 消息（仅在不变量被破坏时触发）。
    pub fn current_function_mut(&mut self) -> &mut IrFunction {
        debug_assert!(
            !self.functions.is_empty(),
            "IrModule invariant violated: functions is empty"
        );
        self.functions.last_mut().expect("at least one function")
    }

    /// 通过 ID 获取函数的可变引用（精确索引，不受函数添加顺序影响）
    ///
    /// # Panics
    ///
    /// 如果 `id.0` 越界（>= `functions.len()`），直接索引会 panic 且无清晰诊断信息。
    /// **生产代码应优先使用 [`IrModule::try_get_function_mut`]**，
    /// 它返回 `Option` 并与 `validate()` 错误模型一致。
    /// 本方法保留是为了向后兼容 display/builder 测试代码，调用方必须确保 id 有效。
    pub fn get_function_mut(&mut self, id: IrFunctionId) -> &mut IrFunction {
        // 边界检查：若越界，panic 时提供清晰诊断信息（比直接下标 panic 友好）
        if (id.0 as usize) >= self.functions.len() {
            panic!(
                "IrModule::get_function_mut: IrFunctionId({}) out of range (functions.len()={}) \
                 — caller must ensure id is valid; prefer try_get_function_mut for fallible \
                 lookup in production code",
                id.0,
                self.functions.len()
            );
        }
        &mut self.functions[id.0 as usize]
    }

    /// 通过 ID 获取函数的可变引用（fallible 版本，返回 `Option`）
    ///
    /// 与 [`IrModule::get_function_mut`] 功能相同，但越界时返回 `None` 而非 panic。
    /// **生产代码应优先使用此方法**，配合 `?` 运算符传播错误，与 `validate()` 错误模型一致。
    ///
    /// # 示例
    ///
    /// ```text
    /// let func = self.module.try_get_function_mut(id)
    ///     .ok_or_else(|| IrBuildError::InternalError {
    ///         what: "function id out of range".to_string(),
    ///         context: format!("id={}, functions.len()={}", id.0, self.module.functions.len()),
    ///         location: SourceLocation::default(),
    ///         hint: "Check function ID management in add_function/build_closure_expr".to_string(),
    ///     })?;
    /// ```
    pub fn try_get_function_mut(&mut self, id: IrFunctionId) -> Option<&mut IrFunction> {
        self.functions.get_mut(id.0 as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vr(id: u32) -> ValueRef {
        ValueRef::new(id)
    }

    #[test]
    fn binary_has_two_sources() {
        let op = IrOp::Binary { dest: vr(0), op: IrBinOp::Add, left: vr(1), right: vr(2) };
        assert_eq!(op.src_value_refs(), vec![vr(1), vr(2)]);
    }

    #[test]
    fn call_has_callee_and_args() {
        let op =
            IrOp::Call { dest: Some(vr(0)), callee: vr(10), args: vec![vr(20), vr(21), vr(22)] };
        assert_eq!(op.src_value_refs(), vec![vr(10), vr(20), vr(21), vr(22)]);
    }

    #[test]
    fn call_with_no_args() {
        let op = IrOp::Call { dest: Some(vr(0)), callee: vr(5), args: vec![] };
        assert_eq!(op.src_value_refs(), vec![vr(5)]);
    }

    #[test]
    fn return_some_value() {
        let op = IrOp::Return { value: Some(vr(42)) };
        assert_eq!(op.src_value_refs(), vec![vr(42)]);
    }

    #[test]
    fn return_none() {
        let op = IrOp::Return { value: None };
        assert!(op.src_value_refs().is_empty());
    }

    #[test]
    fn select_has_three_sources() {
        let op =
            IrOp::Select { condition: vr(1), then_value: vr(2), else_value: vr(3), dest: vr(0) };
        assert_eq!(op.src_value_refs(), vec![vr(1), vr(2), vr(3)]);
    }

    #[test]
    fn load_constant_has_no_sources() {
        let op = IrOp::LoadConstant { dest: vr(0), constant: IrConstant::Number(4.2) };
        assert!(op.src_value_refs().is_empty());
    }

    #[test]
    fn capture_register_source() {
        let op =
            IrOp::Capture { closure: vr(100), index: 0, source: CaptureSource::Register(vr(200)) };
        assert_eq!(op.src_value_refs(), vec![vr(100), vr(200)]);
    }

    #[test]
    fn capture_outer_source() {
        let op =
            IrOp::Capture { closure: vr(100), index: 1, source: CaptureSource::OuterCapture(5) };
        assert_eq!(op.src_value_refs(), vec![vr(100)]);
    }

    #[test]
    fn capture_global_source() {
        let op = IrOp::Capture {
            closure: vr(100),
            index: 2,
            source: CaptureSource::Global(Arc::from("x")),
        };
        assert_eq!(op.src_value_refs(), vec![vr(100)]);
    }

    #[test]
    fn index_set_has_three_sources() {
        let op = IrOp::IndexSet { object: vr(1), index: vr(2), value: vr(3) };
        assert_eq!(op.src_value_refs(), vec![vr(1), vr(2), vr(3)]);
    }

    #[test]
    fn set_local_has_one_source() {
        let op = IrOp::SetLocal { name: Arc::from("x"), value: vr(7) };
        assert_eq!(op.src_value_refs(), vec![vr(7)]);
    }

    #[test]
    fn array_new_collects_all_elements() {
        let op = IrOp::ArrayNew { dest: vr(0), elements: vec![vr(10), vr(11), vr(12)] };
        assert_eq!(op.src_value_refs(), vec![vr(10), vr(11), vr(12)]);
    }

    #[test]
    fn range_new_has_two_sources() {
        let op = IrOp::RangeNew { dest: vr(0), start: vr(1), end: vr(2), inclusive: true };
        assert_eq!(op.src_value_refs(), vec![vr(1), vr(2)]);
    }

    #[test]
    fn jump_has_no_sources() {
        let op = IrOp::Jump { target: BasicBlockId::new(5) };
        assert!(op.src_value_refs().is_empty());
    }

    #[test]
    fn closure_has_no_sources() {
        let op = IrOp::Closure { dest: vr(0), ir_func: IrFunctionId::new(1) };
        assert!(op.src_value_refs().is_empty());
    }

    // ========================================================================
    // P1.2 回归测试：get_function_mut 越界检查
    // ========================================================================

    #[test]
    fn test_get_function_mut_valid_id() {
        // 验证有效 ID 能正常获取函数
        let mut module = IrModule::new();
        let id0 = module.add_function("main");
        let id1 = module.add_function("inner");

        let func0 = module.get_function_mut(id0);
        assert_eq!(func0.name.as_ref(), "main");

        let func1 = module.get_function_mut(id1);
        assert_eq!(func1.name.as_ref(), "inner");
    }

    #[test]
    fn test_get_function_mut_out_of_range_panics() {
        // P1.2 修复：越界访问应 panic 且提供清晰诊断信息
        // 原 bug：直接 `self.functions[id.0 as usize]` 越界 panic 无消息
        // 修复后：显式边界检查 + panic 消息包含 handle 值和 slots.len
        let mut module = IrModule::new();
        let _ = module.add_function("main"); // functions.len() = 1

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            module.get_function_mut(IrFunctionId(99));
        }));

        assert!(result.is_err(), "越界访问应 panic");
        let panic_msg = if let Err(payload) = &result {
            payload
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| payload.downcast_ref::<&'static str>().map(|s| s.to_string()))
                .unwrap_or_default()
        } else {
            String::new()
        };
        assert!(
            panic_msg.contains("IrFunctionId(99)") || panic_msg.contains("99"),
            "panic 消息应包含越界的 ID 值 99，实际: {}",
            panic_msg
        );
        assert!(
            panic_msg.contains("out of range") || panic_msg.contains("越界"),
            "panic 消息应说明越界，实际: {}",
            panic_msg
        );
    }

    #[test]
    fn test_try_get_function_mut_returns_none_for_out_of_range() {
        // P1.2 新增 fallible API：越界返回 None 而非 panic
        let mut module = IrModule::new();
        let id0 = module.add_function("main");

        // 有效 ID 返回 Some
        let func = module.try_get_function_mut(id0);
        assert!(func.is_some(), "有效 ID 应返回 Some");
        assert_eq!(func.unwrap().name.as_ref(), "main");

        // 越界 ID 返回 None
        let func = module.try_get_function_mut(IrFunctionId(99));
        assert!(func.is_none(), "越界 ID 应返回 None");

        // 边界：functions.len() = 1，IrFunctionId(1) 也越界
        let func = module.try_get_function_mut(IrFunctionId(1));
        assert!(func.is_none(), "IrFunctionId(1) 当 functions.len()=1 时应返回 None");
    }

    #[test]
    fn test_try_get_function_mut_allows_chained_query() {
        // 验证 try_get_function_mut 可用于链式查询，不修改 module 状态
        let mut module = IrModule::new();
        let id0 = module.add_function("main");
        let id1 = module.add_function("inner");

        // 第一次查询
        assert!(module.try_get_function_mut(id0).is_some());
        // 第二次查询（验证 borrow 已释放）
        assert!(module.try_get_function_mut(id1).is_some());
        // 越界查询不影响后续
        assert!(module.try_get_function_mut(IrFunctionId(99)).is_none());
        // 仍能正常查询
        assert!(module.try_get_function_mut(id0).is_some());
    }
}
