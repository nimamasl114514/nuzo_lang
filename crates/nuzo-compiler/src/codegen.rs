//! Code Generator — 将 IR 模块转换为目标字节码 (Chunk)
//!
//! ## 职责
//! - 遍历 IrModule.functions，为每个函数生成字节码
//! - 通过 [`RegisterManager`] trait 将 ValueRef (SSA 虚拟寄存器) 映射为物理寄存器 (Reg)
//! - 将 IrConstant 添加到 Chunk 常量池
//! - 将 IrOp 逐一映射为 Instruction 并编码
//!
//! ## 两遍扫描策略
//! 1. **预扫描**：计算每个 BasicBlock 的字节长度 → 确定各块起始偏移
//! 2. **发射**：线性遍历所有指令，跳转目标使用预计算的绝对地址 → 转换为相对 Offset
//!
//! ## 寄存器分配
//! 使用 [`TrackerRegManager`]（引用计数 + 即时回收）：每个 ValueRef 在首次定义时分配寄存器，
//! 最后一次使用时释放回寄存器池供后续复用。相比原始单调递增策略，显著降低 locals_count。

use std::collections::{HashMap, HashSet};

use nuzo_abi::index::SafeIndex;
use nuzo_bytecode::{
    CaptureIdx, CapturedSource, Chunk, ConstIdx, Instruction, Offset, Opcode, OperandKind, Reg, U8,
    U16,
};
use nuzo_core::MAX_FUNCTION_LOCALS;
use nuzo_core::Value;
use nuzo_ir::{
    BasicBlockId, CaptureSource, IrBinOp, IrConstant, IrFunction, IrModule, IrOp, IrUnaryOp,
};
use nuzo_values::NIL;
use nuzo_values::ValueExt;
use nuzo_values::function::FunctionPrototype;
use nuzo_values::heap::HeapObject;
use nuzo_values::nuzo_dict::NuzoDict;

use crate::CompileError;
use crate::allocator::{LsraAllocator, NudConfig, build_intervals, enhance_intervals};
use crate::reg_manager::RegisterManager;

// nuzo_class 宏：用 #[class] 标记结构体，#[class_impl] 标记 impl 块。
// #[method] / #[constructor] / #[get] / #[set] / #[static_method] 是
// #[class_impl] 的辅助属性，由 class_impl 在编译期识别并剥离
// （同时校验 &self/&mut self 签名），无需独立导入。
use nuzo_class::class;

// ============================================================================
// CodegenError — IR 层编译错误
// ============================================================================

/// IR → Bytecode 代码生成阶段的错误
#[derive(Debug, Clone, PartialEq)]
pub enum CodegenError {
    /// 物理寄存器数量超过 u16::MAX
    TooManyRegisters { count: u32 },
    /// 常量池索引超出 u16 范围
    ConstantPoolOverflow,
    /// 跳转目标指向不存在的基本块
    InvalidJumpTarget { target: BasicBlockId },
    /// 跳转偏移量超出 i16 范围
    JumpOffsetOverflow { offset: i64 },
    /// LSRA 寄存器分配失败（区间构建或分配阶段出错）
    LsraFailed { reason: String },
    /// H4: 跳转回填位置超出 chunk.code 边界
    ///
    /// 当 `fixup.pos + fixup.instr_size > code.len()` 时触发,
    /// 通常表示 Phase 2 发射与回填记录不一致(内部 bug 或 IR 损坏)。
    FixupOutOfBounds {
        /// 回填条目记录的跳转指令起始位置
        pos: usize,
        /// 跳转指令字节大小 (3=Jmp, 5=Test)
        instr_size: usize,
        /// 当前 chunk.code 的实际长度
        code_len: usize,
    },
    /// A10: emit_xxx 分发器收到非预期 IrOp 变体
    ///
    /// 当 IR 层新增 IrOp 变体但 codegen 未同步更新时触发,
    /// 替代原 `unreachable!()` panic,使错误可被上层优雅处理。
    UnexpectedIrOp {
        /// IrOp 变体的 Debug 表示(含字段值,便于诊断)
        op: String,
        /// 触发错误的 emit_xxx 函数名(静态字符串)
        context: &'static str,
    },
    /// 通用错误（附带消息）
    Generic { message: String },
    /// 函数参数数量超限（FunctionPrototype.arity 是 u8，最大 255）
    TooManyParameters { count: usize, max: usize },
    /// 闭包捕获变量数量超限（CaptureInfo.capture_index 是 u8，最大 255）
    TooManyCaptures { count: usize, max: usize },
}

impl std::fmt::Display for CodegenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooManyRegisters { count } => {
                write!(f, "Too many registers allocated: {} (max 65535)", count)
            }
            Self::ConstantPoolOverflow => {
                write!(f, "Constant pool overflow (>65535 entries)")
            }
            Self::InvalidJumpTarget { target } => {
                write!(f, "Invalid jump target: bb{}", target.0)
            }
            Self::JumpOffsetOverflow { offset } => {
                write!(f, "Jump offset {} out of i16 range (-32768..32767)", offset)
            }
            Self::LsraFailed { reason } => {
                write!(f, "LSRA register allocation failed: {}", reason)
            }
            Self::FixupOutOfBounds { pos, instr_size, code_len } => {
                write!(
                    f,
                    "Jump fixup out of bounds: pos={} instr_size={} needs pos+size={} but code_len={}",
                    pos,
                    instr_size,
                    pos + instr_size,
                    code_len
                )
            }
            Self::UnexpectedIrOp { op, context } => {
                write!(f, "Unexpected IrOp in {}: {}", context, op)
            }
            Self::Generic { message } => write!(f, "{}", message),
            Self::TooManyParameters { count, max } => {
                write!(f, "too many parameters: {} (max {})", count, max)
            }
            Self::TooManyCaptures { count, max } => {
                write!(f, "too many captured variables: {} (max {})", count, max)
            }
        }
    }
}

impl std::error::Error for CodegenError {}

impl From<CodegenError> for CompileError {
    fn from(err: CodegenError) -> Self {
        // H1 修复: TooManyRegisters 映射到结构化的 TooManyLocals，保留 count 信息，
        // 避免降级为通用 Error 变体（丢失结构化分类）。
        // line/column 设为 0 因 codegen 阶段已无 AST 位置上下文；
        // From<CompileError> for NuzoError 会用 SourceLocation::new(0) 包装，
        // ErrorCode 仍为 CompileError，保留结构化错误类型便于上层处理。
        match err {
            CodegenError::TooManyRegisters { count } => {
                CompileError::TooManyLocals { count: count as usize, line: 0, column: 0 }
            }
            // H1 对齐：参数/捕获超限映射到结构化 CompileError 变体，
            // 避免降级为通用 Error 变体（丢失结构化分类 + 错误码 C0000 不可区分）。
            CodegenError::TooManyParameters { count, max } => {
                CompileError::TooManyParameters { count, max, line: 0, column: 0 }
            }
            CodegenError::TooManyCaptures { count, max } => {
                CompileError::TooManyCapturedVariables { count, max, line: 0, column: 0 }
            }
            other => CompileError::Error { message: other.to_string(), line: 0, column: 0 },
        }
    }
}

impl From<crate::reg_manager::RegAllocError> for CodegenError {
    fn from(err: crate::reg_manager::RegAllocError) -> Self {
        match err {
            crate::reg_manager::RegAllocError::UndefinedValueRef(id) => {
                CodegenError::Generic { message: format!("Undefined ValueRef v{}", id) }
            }
            crate::reg_manager::RegAllocError::PoolExhausted { count } => {
                CodegenError::TooManyRegisters { count: count as u32 }
            }
        }
    }
}

// ============================================================================
// CodeGenerator — 核心结构体
// ============================================================================

/// 跳转回填条目：记录一条待回填的跳转指令
struct JumpFixup {
    /// 跳转指令在 chunk.code 中的字节位置
    pos: usize,
    /// 目标基本块 ID
    target_block: u32,
    /// 跳转指令的字节大小（3=Jmp, 5=Test）
    instr_size: usize,
}

// ============================================================================
// OperandField — 操作数字段语义标注（LSRA 重写安全辅助）
// ============================================================================
//
// 背景：codegen.rs 旧 P2.4 TODO 指出 `rewrite_regs_with_lsra` 在遍历字节码
// 时按 `Opcode::operands()` 返回的 OperandKind 定位 Reg 字段，但部分 opcode
// 的 operands() 布局与实际编码不一致（如 Closure 的 proto_idx 是 ConstIdx
// 但在某些路径被误标为 Reg，或 Capture 等变长编码指令的 tag 字节导致
// operands 求和 < instruction_size）。当 remap 表中 old_reg 与某个 ConstIdx
// 数值相同时，ConstIdx 被误判为 Reg 并被重写 → 常量池索引被破坏。
//
// 本结构仅在 codegen 内部使用，用于：
// 1. 显式标注每个操作数字段的语义类型与字节偏移，避免重写循环中偏移错位
// 2. 配合 `is_remappable_reg` 做防御性范围检查，仅重写 `[0, locals_count)`
//    范围内的寄存器号，避免 ConstIdx 等非 Reg 字段被误改
//
// **不影响字节码序列化格式**：仅是 codegen 内部的辅助结构。
// 完整修复仍需跨 crate 审计 `Opcode::operands()` 实现（见 BACKLOG/issue）。

/// 操作数字段的语义标注
///
/// 记录单个操作数字段的 `OperandKind` 与在指令中的字节偏移，
/// 供 LSRA 重写循环使用，替代裸 `operand_offset` 累加以提升可读性与安全性。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct OperandField {
    /// 字段的字节码布局类型
    kind: OperandKind,
    /// 字段在指令中的字节偏移（相对指令起始，含 opcode 字节）
    offset: usize,
}

/// 解码定长指令的操作数字段布局
///
/// 返回每个操作数字段的 `(kind, offset)` 列表。
/// 仅当 `1 + operands_sum == instr_size` 时返回完整字段列表
/// （即定长编码指令）；变长指令（如 Capture 含 tag 字节、GetGlobalCached
/// 含 ISS pad）返回空 Vec，调用方应跳过重写。
///
/// # 参数
/// * `opcode` - 已解码的 Opcode
///
/// # 返回
/// * `Vec<OperandField>` - 字段列表；空 Vec 表示变长指令
fn decode_operand_fields(opcode: Opcode) -> Vec<OperandField> {
    let operands = opcode.operands();
    let operands_sum: usize = operands.iter().map(|k| k.byte_size()).sum();
    if 1 + operands_sum != opcode.instruction_size() {
        return Vec::new();
    }
    let mut offset: usize = 1; // 跳过 opcode 字节
    let mut fields = Vec::with_capacity(operands.len());
    for &kind in operands {
        fields.push(OperandField { kind, offset });
        offset += kind.byte_size();
    }
    fields
}

/// 防御性范围检查：验证 old_reg 值是否在已分配寄存器范围内
///
/// LSRA 重写时，若 `Opcode::operands()` 分类错误（例如把 ConstIdx 误标为 Reg），
/// `old_reg` 的数值可能与某个常量池索引相同。通过限制只重写
/// `[0, locals_count)` 范围内的寄存器号，可避免误改 ConstIdx 字段
/// （常量池索引通常远大于 locals_count，且超出已分配寄存器编号空间）。
///
/// # 参数
/// * `value` - 从字节码读出的 u16 值（可能是 Reg 或被误标的 ConstIdx）
/// * `locals_count` - 当前 chunk 的已分配寄存器数量上界
///
/// # 返回
/// * `true` - 值在 `[0, locals_count)` 范围内，可安全视为 Reg 重写
/// * `false` - 值超出范围，可能是被误标的非 Reg 字段，应跳过
fn is_remappable_reg(value: u16, locals_count: u16) -> bool {
    value < locals_count
}

/// Code Generator：将 IrModule（IR 中间表示）转换为 Chunk（字节码）
///
/// # 使用流程
/// ```ignore
/// let mut codegen = CodeGenerator::new();
/// let chunk = codecodegen.generate(&ir_module)?;
/// ```
#[class]
pub struct CodeGenerator {
    /// 正在构建的字节码块
    chunk: Chunk,
    /// 统一寄存器管理器（每次 generate_function 重建）
    ///
    /// 使用 `Option` 是因为 CodeGenerator 在 `new()` 时不持有管理器，
    /// 而是在每次 `generate_function()` 开头构建并注入。
    /// 通过 `rm()` 辅助方法获取可变引用。
    reg_manager: Option<Box<dyn RegisterManager>>,
    /// 基本块起始字节偏移（block_id → 在 chunk.code 中的起始位置）
    /// 由预扫描阶段填充，用于调试/诊断。跳转目标使用 jump_fixups 回填机制。
    block_starts: HashMap<u32, usize>,
    /// 跳转回填列表
    ///
    /// Phase 2 发射时，由于 LoadArg/Mov 等指令存在条件发射（dest==src 时省略），
    /// 预扫描的 block_starts 可能与实际位置不一致。因此跳转偏移采用回填策略：
    /// 先发射占位(offset=0)，所有块发射完毕后用实际位置回填。
    jump_fixups: Vec<JumpFixup>,
    /// LSRA def IP 收集：def_ips[reg] = 首次 def 该寄存器的字节码 IP
    ///
    /// 索引为物理寄存器编号，值为该寄存器首次被分配时 `chunk.code.len()` 的快照。
    /// `build_intervals` 据此构建活跃区间起点。
    /// 保留供 apply_lsra 使用，但通过 reg_manager 分配时不再自动填充。
    #[allow(dead_code)]
    def_ips: [Option<usize>; MAX_FUNCTION_LOCALS as usize],
    /// LSRA use IP 收集：use_ips[reg] = 最后一次 use 该寄存器的字节码 IP
    ///
    /// 保留供 apply_lsra 使用，但通过 reg_manager 分配时不再自动填充。
    #[allow(dead_code)]
    use_ips: [Option<usize>; MAX_FUNCTION_LOCALS as usize],
    /// 当前是否正在生成顶层 main 函数（函数索引 0）
    ///
    /// main 函数的 `IrOp::Return` 终止符映射为 `Opcode::Halt` 而非
    /// `Opcode::Return`，因为顶层代码没有调用者帧可弹出。
    is_main_function: bool,
    /// IrFunctionId.0 -> 常量池索引 (ConstIdx) 的映射表
    ///
    /// 在 `generate()` 的 Phase 1 中，为每个非 main 的 IrFunction 生成独立的
    /// FunctionPrototype + HeapObject::Closure，存入 main chunk 的常量池，
    /// 并记录其常量池索引。当遇到 `IrOp::Closure { ir_func }` 时，
    /// 通过此映射表查找正确的常量池索引，确保 VM 的 `Opcode::Closure`
    /// 能从常量池取出 Closure 对象（而非错误的浮点数）。
    closure_indices: HashMap<u32, u16>,
    /// 局部变量名 → ValueRef 映射表
    ///
    /// 追踪 IR 层 SetLocal 指令建立的变量绑定，
    /// 供后续 GetLocal 指令查找并发射 Mov 实现跨基本块的值传递。
    /// 每个函数开始时清空，确保作用域隔离。
    ///
    /// 存储物理寄存器编号（非 ValueRef），因为 SetLocal/GetLocal 的
    /// 隐式消费不计入 usage_counter（src_value_refs 对 GetLocal 返回空）。
    /// 改为存储寄存器避免了在已消费 ValueRef 上重复调用 consume_use。
    local_map: std::collections::HashMap<std::sync::Arc<str>, u16>,
    /// 模块路径（字符串）→ init_flag_slot 映射（同模块多次导入共享 slot）
    ///
    /// 与 AST 路径 `Compiler::init_flag_slots` 对应。
    /// key 是 `ImportRecord.path.to_string_lossy()` 的结果，
    /// 与 VM `module_cache` 的 key 一致。
    init_flag_slots: HashMap<String, u16>,
    /// 下一个可用的 init_flag_slot（从 0 递增）
    next_init_flag_slot: u16,
    /// lazy import 符号名 → (模块路径字符串, slot) 映射
    ///
    /// 当 `emit_variable_access` 的 `GetGlobal` 引用的 name 命中此映射时，
    /// 在发射 GetGlobal 之前先发射 `InitModule`（精确延迟发射）。
    lazy_symbol_map: HashMap<String, (String, u16)>,
    /// 已发射 InitModule 的 lazy 模块路径集合（避免重复发射）
    emitted_lazy_modules: HashSet<String>,
    /// eager import 列表：(模块路径字符串, slot)
    ///
    /// 在 `generate` 的 Phase 2 之前（main 函数字节码最前面）统一发射。
    eager_imports: Vec<(String, u16)>,
    /// 子模块 main Chunk 列表：(模块路径字符串, Chunk)
    ///
    /// 由 `process_imports` 从 `ImportRecord.functions` 生成，
    /// 通过 `take_sub_module_chunks` 传给调用方注册到 VM `module_cache`。
    sub_module_chunks: Vec<(String, Chunk)>,
}

impl CodeGenerator {
    /// 创建新的 Code Generator（空状态）
    pub fn new() -> Self {
        Self {
            chunk: Chunk::new(),
            reg_manager: None,
            block_starts: HashMap::new(),
            jump_fixups: Vec::new(),
            def_ips: [None; MAX_FUNCTION_LOCALS as usize],
            use_ips: [None; MAX_FUNCTION_LOCALS as usize],
            is_main_function: false,
            closure_indices: HashMap::new(),
            local_map: HashMap::new(),
            init_flag_slots: HashMap::new(),
            next_init_flag_slot: 0,
            lazy_symbol_map: HashMap::new(),
            emitted_lazy_modules: HashSet::new(),
            eager_imports: Vec::new(),
            sub_module_chunks: Vec::new(),
        }
    }

    // ── 公开 API ──

    /// 主入口：将 IR Module 完整转换为 Chunk
    ///
    /// # 两阶段生成策略
    ///
    /// ## Phase 1: 为每个非 main 的 IrFunction 生成独立 Chunk + FunctionPrototype
    ///
    /// 遍历 `module.functions`，对索引 > 0 的函数（非 main）：
    /// 1. 递归调用 `generate_function()` 生成独立的 Chunk
    /// 2. 从 Chunk 构建 `FunctionPrototype`（包含独立的 code/constants/lines）
    /// 3. 创建 `HeapObject::Closure { prototype, captured: [] }` 对象
    /// 4. 将 Closure 对象存入 main chunk 的常量池
    /// 5. 记录 `IrFunctionId -> ConstIdx` 映射到 `closure_indices`
    ///
    /// 这确保 VM 的 `Opcode::Closure` 能从常量池取出 Closure 对象
    /// （而非错误的浮点数），与旧编译器 (compiler.rs) 的行为一致。
    ///
    /// ## Phase 2: 为 main 函数生成字节码
    ///
    /// main 函数（索引 0）的字节码直接写入当前 chunk，
    /// 其 `IrOp::Return` 映射为 `Opcode::Halt`。
    pub fn generate(&mut self, module: &IrModule) -> Result<Chunk, CodegenError> {
        // Phase 0: 处理 lazy imports — 生成子模块 Chunk + 符号映射
        //
        // Eager import 已由 IrBuilder::build_with_resolver 递归编译并合并到主模块 IR，
        // 此处仅处理 lazy import：
        // - 从 `record.functions` 构造子模块 IrModule 并生成独立 Chunk（存入 sub_module_chunks）
        // - 记录 resolved_symbols → lazy_symbol_map（GetGlobal 时精确发射 InitModule）
        self.process_imports(module)?;

        // Phase 1: 为每个非 main 的 IrFunction 生成 FunctionPrototype + Closure 对象
        //
        // 🔧 关键修复（两层策略）：
        //
        // 1. **反向序处理**：按 ID 降序处理子函数。
        //    由于 `build_closure_expr` 中 `module.add_function()` 是追加操作，
        //    内层闭包的 ID 总是大于外层（如 outer=1, inner=2）。
        //    先处理 inner 再处理 outer，确保 outer 引用 inner 时 inner 已注册。
        //
        // 2. **共享 closure_indices**：`generate_sub_function` 创建的子 CodeGenerator
        //    默认有空 closure_indices，无法解析对兄弟函数的 Closure 引用。
        //    修复：将当前（已部分填充的）closure_indices 传递给子生成器，
        //    使子函数体内的 `IrOp::Closure { ir_func: N }` 能找到 N 的常量池索引。
        //
        // 容错：单个函数注册失败不阻断后续函数（continue 而非 ?）。
        let mut non_main: Vec<&IrFunction> =
            module.functions.iter().filter(|f| f.id.0 != 0).collect();
        non_main.sort_by_key(|f| std::cmp::Reverse(f.id.0));

        for func in &non_main {
            if let Err(e) = self.register_sub_function(func) {
                // 结构化硬错误立即返回（参数/捕获超限是源代码错误，容错无意义，
                // 立即返回能让用户看到精确错误而非 Phase 2 的 "未注册函数 ID" 兜底）。
                // 其他错误（Generic 等）保持容错 continue，对齐 L413 注释设计意图。
                match &e {
                    CodegenError::TooManyParameters { .. }
                    | CodegenError::TooManyCaptures { .. } => return Err(e),
                    _ => {
                        eprintln!(
                            "[Codegen] Warning: failed to register sub-function '{}' (ID={}): {}",
                            func.name, func.id.0, e
                        );
                        continue;
                    }
                }
            }
        }

        // Phase 1.5: 发射 eager import 的 InitModule（在 main 函数字节码最前面）
        //
        // eager import 在程序开始时立即初始化子模块（执行顶层副作用代码）。
        // 这些指令在 main 函数字节码之前，VM 从 IP=0 开始执行时会先执行它们。
        // 先 clone 避免 &self.eager_imports 的不可变借用与 emit_init_module 的 &mut self 冲突
        let eager = self.eager_imports.clone();
        for (path, slot) in &eager {
            self.emit_init_module(path, *slot)?;
        }

        // Phase 2: 为 main 函数生成字节码
        if let Some(main_func) = module.functions.first() {
            self.is_main_function = true;
            self.generate_function(main_func)?;
            self.is_main_function = false;
        }

        // === LSRA 后处理（暂时禁用）===
        // P2.4 TODO（已缓解）：rewrite_regs_with_lsra 已通过 `OperandField` 辅助结构
        // 与 `is_remappable_reg` 防御性范围检查缓解 ConstIdx 误判为 Reg 的问题
        // （见 `decode_operand_fields` / `is_remappable_reg`）。但完整修复仍需跨
        // crate 审计 `Opcode::operands()` 实现，因此 apply_lsra 仍保持禁用。
        //
        // 根因分析：
        //   rewrite_regs_with_lsra 遍历字节码，按 `Opcode::operands()` 返回的 OperandKind
        //   分类定位 Reg 字段。但部分 opcode 的 operands() 布局与实际编码不一致
        //   （例如 Closure 的 proto_idx 是 ConstIdx 但在某些路径被误标为 Reg，
        //   或 Capture 等变长编码指令的 tag 字节导致 operands 求和 < instruction_size）。
        //   当 remap 表中恰好有 old_reg 与某个 ConstIdx 数值相同时，
        //   ConstIdx 被误判为 Reg 并被重写 → 常量池索引被破坏 → ConstantOutOfBounds。
        //
        // 当前缓解策略（已实施）：
        //   - `decode_operand_fields` 显式标注每个字段 (kind, offset)，避免偏移错位
        //   - `is_remappable_reg(old_reg, locals_count)` 范围检查：仅重写
        //     `[0, locals_count)` 范围内的值，常量池索引通常远大于 locals_count，
        //     被范围检查跳过，避免误改破坏常量池
        //   - 仍保留：仅当 `1 + operands_sum == instr_size` 时才信任 operands 布局
        //
        // 完整修复方向（跨 crate，需在 nuzo-bytecode/nuzo-opcode 中审计）：
        //   1. 审计 `Opcode::operands()` 实现，确保每条指令的 OperandKind 分类准确
        //      （特别是 Closure/Capture/GetGlobal 等含 ConstIdx 的指令）
        //   2. 补闭包常量回归测试：构造 IrModule 包含嵌套闭包 + 父级常量池中有
        //      与 proto_idx 数值相同的 reg 号，验证重写后 proto_idx 未被破坏
        //   3. 启用后跑全量 e2e + integration tests 确认无 ConstantOutOfBounds
        //
        // 风险评估：启用 apply_lsra 会重写已分配的物理寄存器号，可能与 DualPool
        // 单端布局（top 递增保证不冲突）的假设冲突。需确认 LSRA 重映射后寄存器
        // 仍满足 DualPool 不变量（持久区单调递增、临时区可复用）。
        //
        // self.apply_lsra()?;

        // 设置 locals_count 为 reg_manager 报告的峰值寄存器数
        if let Some(mgr) = self.reg_manager.take() {
            self.chunk.locals_count = mgr.finalize();
        } else {
            self.chunk.locals_count = 0;
        }

        Ok(self.chunk.clone())
    }

    /// 取走生成的子模块 Chunks（供调用方注册到 VM `module_cache`）
    ///
    /// 返回 `(模块路径字符串, Chunk)` 列表。路径字符串与 `InitModule`
    /// 常量池中的路径一致，即 `ImportRecord.path.to_string_lossy()`。
    pub fn take_sub_module_chunks(&mut self) -> Vec<(String, Chunk)> {
        std::mem::take(&mut self.sub_module_chunks)
    }

    // ── InitModule 发射（IR 路径 import 集成）──

    /// 为模块路径分配 `init_flag_slot`（同模块多次导入共享 slot）
    fn allocate_init_flag_slot(&mut self, path: &str) -> u16 {
        if let Some(&slot) = self.init_flag_slots.get(path) {
            return slot;
        }
        let slot = self.next_init_flag_slot;
        self.next_init_flag_slot = self.next_init_flag_slot.wrapping_add(1);
        self.init_flag_slots.insert(path.to_string(), slot);
        slot
    }

    /// 发射 `InitModule` 指令到当前 chunk
    ///
    /// 字节码格式（5 字节）：`[Opcode::InitModule] [module_idx:u16 LE] [init_flag_slot:u16 LE]`
    fn emit_init_module(&mut self, path: &str, slot: u16) -> Result<(), CodegenError> {
        let module_idx = self
            .chunk
            .try_add_constant(Value::from_string(path))
            .map_err(|_| CodegenError::ConstantPoolOverflow)?;
        self.chunk.emit(Instruction::InitModule {
            module_idx: ConstIdx(Self::narrow_u16(module_idx)?),
            init_flag_slot: U16(slot),
        });
        Ok(())
    }

    /// 检查符号是否匹配某个 lazy import 的 `resolved_symbols`，
    /// 如果匹配且尚未发射 InitModule，则立即发射（精确延迟发射）。
    ///
    /// 在 `emit_variable_access` 的 `GetGlobal` 处理中调用，
    /// 确保子模块顶层代码在符号首次引用时执行。
    fn try_emit_lazy_init_for_symbol(&mut self, symbol: &str) -> Result<(), CodegenError> {
        // 先 clone 避免 lazy_symbol_map.get 的不可变借用与后续 &mut self 冲突
        let entry = self.lazy_symbol_map.get(symbol).cloned();
        if let Some((path, slot)) = entry
            && !self.emitted_lazy_modules.contains(&path)
        {
            self.emitted_lazy_modules.insert(path.clone());
            self.emit_init_module(&path, slot)?;
        }
        Ok(())
    }

    /// 处理 `IrModule.imports` — 仅处理 lazy import（生成子模块 Chunk + 符号映射）
    ///
    /// **Eager import 已由 IrBuilder::build_with_resolver 递归编译并合并到主模块 IR**，
    /// 此处不再重复处理。只对 `lazy: true` 的 ImportRecord：
    /// 1. 分配 `init_flag_slot`
    /// 2. 从 `record.functions` 构造子模块 IrModule，生成独立 Chunk
    /// 3. 记录 `resolved_symbols` → `lazy_symbol_map`
    fn process_imports(&mut self, module: &IrModule) -> Result<(), CodegenError> {
        for record in &module.imports {
            // Eager imports are already handled by IrBuilder::build_with_resolver
            // which recursively compiled and merged sub-module functions into the main IR.
            // Only lazy imports need sub-module chunk generation and deferred InitModule.
            if !record.lazy {
                continue;
            }

            let path_str = record.path.to_string_lossy().into_owned();
            let slot = self.allocate_init_flag_slot(&path_str);

            // 为子模块生成独立 Chunk（含 main 顶层代码 + 子函数 FunctionPrototype）
            // 仅当子模块有函数定义时才生成（空 import 不需要 Chunk）
            if !record.functions.is_empty() {
                let mut sub_module = IrModule::new();
                sub_module.functions = record.functions.clone();
                match Self::generate_sub_module_chunk(&sub_module) {
                    Ok(chunk) => {
                        self.sub_module_chunks.push((path_str.clone(), chunk));
                    }
                    Err(e) => {
                        eprintln!(
                            "[Codegen] Warning: failed to generate sub-module chunk for '{}': {}",
                            path_str, e
                        );
                    }
                }
            }

            // lazy import: 记录符号 → (path, slot) 映射
            for symbol in &record.resolved_symbols {
                self.lazy_symbol_map.insert(symbol.clone(), (path_str.clone(), slot));
            }
        }
        Ok(())
    }

    /// 为子模块生成独立 Chunk（用于 `InitModule` 运行期执行）
    ///
    /// 与 [`generate`](Self::generate) 的区别：
    /// - `is_main_function = false`：main 函数的 Return → `Opcode::Return`（非 Halt），
    ///   因为子模块通过帧切换（`execute_module_toplevel`）执行，
    ///   `OP_RETURN` 触发 `pop_frame` 恢复 caller 状态
    /// - 不处理 imports（子模块的 import 已在 `IrBuilder::resolve_imports` 递归处理）
    fn generate_sub_module_chunk(module: &IrModule) -> Result<Chunk, CodegenError> {
        let mut sub_gen = CodeGenerator::new();
        // is_main_function = false：Return → Return（非 Halt）
        sub_gen.is_main_function = false;

        // Phase 1: 为非 main 函数生成 FunctionPrototype + Closure
        let mut non_main: Vec<&IrFunction> =
            module.functions.iter().filter(|f| f.id.0 != 0).collect();
        non_main.sort_by_key(|f| std::cmp::Reverse(f.id.0));
        for func in &non_main {
            if let Err(e) = sub_gen.register_sub_function(func) {
                eprintln!(
                    "[Codegen] Warning: failed to register sub-function '{}' (ID={}): {}",
                    func.name, func.id.0, e
                );
                continue;
            }
        }

        // Phase 2: 为 main 函数生成字节码（Return → Return）
        if let Some(main_func) = module.functions.first() {
            sub_gen.generate_function(main_func)?;
        }

        // 设置 locals_count
        if let Some(mgr) = sub_gen.reg_manager.take() {
            sub_gen.chunk.locals_count = mgr.finalize();
        } else {
            sub_gen.chunk.locals_count = 0;
        }

        Ok(sub_gen.chunk)
    }

    /// 消费 CodeGenerator，返回生成的 Chunk（避免 clone）
    pub fn into_chunk(mut self) -> Chunk {
        if let Some(mgr) = self.reg_manager.take() {
            self.chunk.locals_count = mgr.finalize();
        } else {
            self.chunk.locals_count = 0;
        }
        self.chunk
    }

    /// 获取 Chunk 的不可变引用
    pub fn chunk(&self) -> &Chunk {
        &self.chunk
    }

    // ── 函数级生成 ──

    /// 为非 main 的 IrFunction 生成独立的 Chunk 并注册到 closure_indices
    ///
    /// 从 `generate()` Phase 1 循环中提取，使单个函数注册失败不阻断后续函数。
    /// 完整流程：生成子函数字节码 → 构建 FunctionPrototype → 创建 Closure 对象
    /// → 存入常量池 → 记录 ID 映射。
    fn register_sub_function(&mut self, func: &IrFunction) -> Result<(), CodegenError> {
        // 前置检查：参数/捕获数量超限时尽早返回结构化错误，
        // 避免走到 narrow_u8 兜底降级为 Generic（对齐 project_rules 第八节：
        // 编译错误不能降级为 C0000 通用 Error）。
        if func.params.len() > u8::MAX as usize {
            return Err(CodegenError::TooManyParameters {
                count: func.params.len(),
                max: u8::MAX as usize,
            });
        }
        if func.captures.len() > u8::MAX as usize {
            return Err(CodegenError::TooManyCaptures {
                count: func.captures.len(),
                max: u8::MAX as usize,
            });
        }

        let sub_chunk = self.generate_sub_function(func)?;

        let (code, constants, lines, debug_info, locals_count, spill_slot_count) =
            sub_chunk.into_parts();

        let captured_vars: Vec<nuzo_values::heap::CaptureInfo> = func
            .captures
            .iter()
            .enumerate()
            .map(|(idx, desc)| {
                Ok(nuzo_values::heap::CaptureInfo {
                    name: desc.name.to_string(),
                    mode: if desc.is_mutable {
                        nuzo_values::heap::CaptureMode::ByBox
                    } else {
                        nuzo_values::heap::CaptureMode::ByValue
                    },
                    capture_index: Self::narrow_u8(idx)?,
                })
            })
            .collect::<Result<_, CodegenError>>()?;

        let prototype = FunctionPrototype::new(
            func.name.to_string(),
            Self::narrow_u8(func.params.len())?,
            locals_count,
            code,
            constants,
            captured_vars,
            lines,
            debug_info,
            spill_slot_count,
        );

        // 创建 Closure 对象（captured 为空，运行时由 Capture 指令填充）
        let closure_value = Value::from_heap_object_gc(HeapObject::Closure {
            prototype: std::sync::Arc::new(prototype),
            captured: Vec::new(),
            parent_env: None,
        });

        // C1 增强: 使用 try_add_constant (Result API) 替代 add_constant + 手动检查,
        // 消除死代码(原 add_constant 会在溢出时 panic,后续检查永不触发)。
        let const_idx = self
            .chunk
            .try_add_constant(closure_value)
            .map_err(|_| CodegenError::ConstantPoolOverflow)?;

        self.closure_indices.insert(func.id.0, Self::narrow_u16(const_idx)?);

        Ok(())
    }

    /// 为非 main 的 IrFunction 生成独立的 Chunk
    ///
    /// 创建一个临时的 CodeGenerator 实例，为该函数生成独立的字节码块。
    /// 临时实例拥有自己的 chunk、block_starts 等状态，
    /// 与 main chunk 完全隔离，避免跨函数状态污染。
    ///
    /// 🔧 关键修复：将父级 CodeGenerator 的 closure_indices 共享给子生成器。
    /// 子函数体中可能包含 `IrOp::Closure { ir_func: N }` 引用其他子函数（如 outer 引用 inner），
    /// 如果子生成器的 closure_indices 为空，则无法解析这些引用导致 "未注册的函数 ID" 错误。
    fn generate_sub_function(&self, func: &IrFunction) -> Result<Chunk, CodegenError> {
        let mut sub_gen = CodeGenerator::new();
        // 非顶层函数，Return 映射为 Opcode::Return（非 Halt）
        sub_gen.is_main_function = false;

        // 🔧 P1.1 修复（嵌套闭包 O(N²) 拷贝）：
        // 旧实现 `for &c in self.chunk.constants() { sub_gen.try_add_constant(c) }` 全量 clone
        // 父级常量池，深度嵌套场景下子-子-子...每层都复制父级全部常量，O(N²)。
        //
        // 修复策略：子函数真正需要的只是父级常量池中 HeapObject::Closure 类型常量
        // （只有 IrOp::Closure 指令会以 proto_idx 索引常量池；其他常量如 Number/String
        // 子函数会通过 add_constant 自行添加）。因此：
        // 1) 遍历 closure_indices 收集 (ir_func_id, parent_proto_idx) 对
        // 2) 按 parent_proto_idx 升序排序，依次从父级常量池取出 Closure 常量添加到子 chunk
        // 3) 同时构建 ir_func_id → 子 chunk 新 proto_idx 的重映射表
        //
        // 复杂度：子 chunk 只包含 M 个 Closure 常量（M = 闭包数，通常远小于 N = 总常量数），
        // 嵌套 K 层为 O(M*K)，远小于原 O(N²)。
        //
        // 边界处理：若 parent_proto_idx 越界（父级常量池未含此 Closure），返回带诊断信息的错误。
        let mut closure_entries: Vec<(u32, u16)> =
            self.closure_indices.iter().map(|(&id, &parent_idx)| (id, parent_idx)).collect();
        // 按 parent_proto_idx 升序排序，保证子 chunk 中常量添加顺序稳定可复现
        closure_entries.sort_by_key(|&(_, parent_idx)| parent_idx);

        let mut new_closure_indices: HashMap<u32, u16> = HashMap::new();
        for (ir_func_id, parent_proto_idx) in closure_entries {
            // 从父级 chunk 常量池取出 Closure 常量
            let closure_const =
                self.chunk.get_constant(parent_proto_idx as usize).ok_or_else(|| {
                    CodegenError::Generic {
                        message: format!(
                            "generate_sub_function: parent_proto_idx {} not found in parent chunk \
                         constants (len={}) — indicates closure_indices table is stale or \
                         parent chunk was not fully built before sub-function generation",
                            parent_proto_idx,
                            self.chunk.constants().len()
                        ),
                    }
                })?;
            // 添加到子 chunk（try_add_constant 会自动去重）
            let new_idx = sub_gen
                .chunk
                .try_add_constant(closure_const)
                .map_err(|_| CodegenError::ConstantPoolOverflow)?;
            new_closure_indices.insert(ir_func_id, Self::narrow_u16(new_idx)?);
        }
        sub_gen.closure_indices = new_closure_indices;

        // 设置 debug_info 中的函数名
        std::sync::Arc::make_mut(&mut sub_gen.chunk.debug_info).function_name =
            Some(func.name.to_string());

        sub_gen.generate_function(func)?;

        if let Some(mgr) = sub_gen.reg_manager.take() {
            sub_gen.chunk.locals_count = mgr.finalize();
        } else {
            sub_gen.chunk.locals_count = 0;
        }
        Ok(sub_gen.chunk)
    }

    /// 生成单个 IrFunction 的字节码
    ///
    /// 内部执行两遍扫描：
    /// 1. **预扫描**：基于指令大小估算，计算每个基本块的绝对起始字节偏移
    ///    （含函数在 chunk 中的起始偏移，支持多函数顺序拼接）
    /// 2. **发射**：逐条翻译 IrOp 为 Instruction，跳转使用回填机制保证正确性
    ///
    /// # 跳转偏移正确性保证（回填策略）
    /// 由于 LoadArg/Mov 等指令存在条件发射（dest==src 时省略），Phase 1 预扫描的
    /// 块起始地址可能与实际位置不一致。因此采用回填策略：
    /// - Phase 2 发射跳转时先写占位(offset=0)，记录 (位置, 目标块, 指令大小)
    /// - Phase 2 结束后统一用实际块位置回填所有跳转偏移
    fn generate_function(&mut self, func: &IrFunction) -> Result<(), CodegenError> {
        // 构建寄存器管理器：仅当外部未注入时才创建默认的 TrackerRegManager
        if self.reg_manager.is_none() {
            let use_counts = crate::usage_counter::count_usages_with_loop_protection(func);
            let reg_mgr = crate::reg_manager::TrackerRegManager::new(use_counts);
            self.reg_manager = Some(Box::new(reg_mgr));
        }

        // 函数在 chunk 中的起始偏移（支持多函数顺序拼接）
        let func_start = self.chunk.len();

        // Phase 1: 预扫描，计算每个基本块的绝对起始偏移（近似值，用于诊断）
        // 实际跳转目标使用 Phase 2 回填机制
        self.block_starts.clear();
        self.local_map.clear();
        self.jump_fixups.clear();
        let mut current_offset = func_start;
        for block in &func.blocks {
            self.block_starts.insert(block.id.0, current_offset);
            for op in &block.instructions {
                current_offset += self.estimate_instruction_size(op);
            }
        }

        // Phase 2: 发射所有基本块的指令，记录实际块位置，跳转使用回填
        for block in &func.blocks {
            // 记录该基本块的实际起始位置（覆盖 Phase 1 的近似值）
            let actual_start = self.chunk.len();
            self.block_starts.insert(block.id.0, actual_start);

            for op in &block.instructions {
                self.emit_op(op)?;
            }
        }

        // Phase 3: 回填所有跳转偏移
        self.resolve_jump_fixups()?;

        Ok(())
    }

    /// 估算单条 IrOp 编码后的字节数（用于预扫描）
    ///
    /// **关键约束**：估算值必须与 `emit_op` 实际发射的字节数完全一致，
    /// 否则预扫描的块起始偏移会与发射时实际位置错位，导致跳转目标错误。
    ///
    /// 编码格式：opcode(1) + Σ operand.byte_size()
    /// - Reg/ConstIdx/Offset/U16/CaptureIdx: 2 字节
    /// - U8: 1 字节
    fn estimate_instruction_size(&self, op: &IrOp) -> usize {
        match op {
            IrOp::LoadConstant { .. } => 5, // LoadK: 1+2+2
            IrOp::LoadArg { .. } => 5,      // Mov: 1+2+2
            IrOp::Binary { .. } => 7,       // 算术/比较: 1+2+2+2
            IrOp::Unary { .. } => 5,        // Neg/Not: 1+2+2
            IrOp::Mov { .. } => 5,          // Mov: 1+2+2（dest==src 时省略，但保守估算）
            IrOp::Call { .. } => 4,         // Call: 1+2+1 (argc as U8)
            // 注意：Call 可能额外发射 N 条 Mov（参数移动），但目标寄存器与参数寄存器
            // 不同时才发射。预扫描无法静态判断，保守估算为 4（不含 Mov）。
            // 这会导致预扫描偏小，但 Phase 1 的简化寄存器分配通常使 dest==src，
            // 实际 Mov 很少触发。后续若启用寄存器复用需重新评估。
            IrOp::Closure { .. } => 5,    // Closure: 1+2+2
            IrOp::Capture { .. } => 7,    // Capture: 1+2+2+2 (closure+idx+source)
            IrOp::GetLocal { .. } => 5,   // Mov: dest(2) + src(2) + opcode(1)
            IrOp::SetLocal { .. } => 5,   // 可能发射 Mov(5) 写回旧寄存器（循环变量更新）
            IrOp::GetGlobal { .. } => 7,  // GetGlobal: 1+2+2+2(ISS pad)
            IrOp::SetGlobal { .. } => 5,  // SetGlobal: 1+2+2
            IrOp::GetCapture { .. } => 5, // GetCaptured: 1+2+2
            IrOp::SetCapture { .. } => 5, // SetCaptured: 1+2+2
            IrOp::Jump { .. } => 3,       // Jmp: 1+2
            IrOp::JumpIf { .. } => 8,     // Test(5) + Jmp(3)
            IrOp::Return { value } => {
                // 顶层 main 函数：Return → Halt（1 字节，丢弃返回值）
                if self.is_main_function {
                    return 1;
                }
                // 有值: Return(1+2=3)
                // 无值: LoadNil(1+2=3) + Return(1+2=3) = 6
                match value {
                    Some(_) => 3,
                    None => 6,
                }
            }
            IrOp::ArrayNew { dest: _, elements } => {
                // ArrayNew(5) + N * (LoadK(5) + SetIndex(7)) = 5 + N*12
                5 + elements.len() * 12
            }
            IrOp::ObjectNew { .. } => 5, // LoadK: 1+2+2 (dest+constant)
            IrOp::GetField { .. } => 7,  // GetProp: 1+2+2+2
            IrOp::SetField { .. } => 7,  // SetProp: 1+2+2+2
            IrOp::IndexGet { .. } => 7,  // GetIndex: 1+2+2+2
            IrOp::IndexSet { .. } => 7,  // SetIndex: 1+2+2+2
            IrOp::IndexSetMut { .. } => 7, // SetIndexMut: 1+2+2+2
            IrOp::Select { .. } => 18,   // Select: Test(5)+Mov(5)+Jmp(3)+Mov(5)
            IrOp::RangeNew { .. } => 8,  // RangeNew: 1+2+2+2+1 (dest+start+end+inclusive)
            IrOp::TryStart { .. } => 4,  // TryStart: 1+2+1 (catch_offset+exception_reg)
            IrOp::TryEnd => 1,           // TryEnd: 仅 opcode
            IrOp::Out { .. } => 3,       // Out: 1+2 (value_reg)
            IrOp::Print { .. } => 3,     // Print: 1+2
            IrOp::Len { .. } => 5,       // Len: 1+2+2 (dest+object)
            IrOp::StringBuild { operands, .. } => {
                // StringBuild(7) + N * Mov(5) (操作数排列)
                7 + operands.len() * 5
            }
            IrOp::SliceChainInit { .. } => 3, // SliceChainNew: 1+2 (dest)
            IrOp::SliceChainAppend { .. } => 5, // SliceChainAppend: 1+2+2 (chain+src)
            IrOp::SliceChainFinish { .. } => 5, // SliceChainFinish: 1+2+2 (dest+chain)
        }
    }

    // ── 指令发射核心 ──

    /// 将单条 IrOp 翻译为一或多条 Instruction 并写入 Chunk
    ///
    /// # 分发器架构
    /// 本方法仅做变体类别路由，实际发射逻辑由 9 个语义分组的
    /// `#[method]` 辅助方法承担。每个辅助方法只处理一类语义相关的
    /// IrOp 变体，便于独立维护与扩展。
    ///
    /// ## 分组一览
    /// | 辅助方法 | 处理的 IrOp 变体 |
    /// |---------|-----------------|
    /// | `emit_literal_load` | LoadConstant, LoadArg, Mov |
    /// | `emit_arithmetic` | Binary, Unary |
    /// | `emit_call_closure` | Call, Closure |
    /// | `emit_variable_access` | GetLocal, SetLocal, GetGlobal, SetGlobal, GetCapture, SetCapture |
    /// | `emit_control_flow` | Jump, JumpIf, Return |
    /// | `emit_composite_types` | ArrayNew, ObjectNew, RangeNew |
    /// | `emit_property_access` | GetField, SetField, IndexGet, IndexSet |
    /// | `emit_exception_handling` | TryStart, TryEnd, Out |
    /// | `emit_debug` | Print |
    fn emit_op(&mut self, op: &IrOp) -> Result<(), CodegenError> {
        match op {
            // ── 字面量加载与寄存器移动 ──
            IrOp::LoadConstant { .. } | IrOp::LoadArg { .. } | IrOp::Mov { .. } => {
                self.emit_literal_load(op)
            }
            // ── 算术运算 ──
            IrOp::Binary { .. } | IrOp::Unary { .. } => self.emit_arithmetic(op),
            // ── 函数调用与闭包 ──
            IrOp::Call { .. } | IrOp::Closure { .. } | IrOp::Capture { .. } => {
                self.emit_call_closure(op)
            }
            // ── 变量访问（局部/全局/捕获）──
            IrOp::GetLocal { .. }
            | IrOp::SetLocal { .. }
            | IrOp::GetGlobal { .. }
            | IrOp::SetGlobal { .. }
            | IrOp::GetCapture { .. }
            | IrOp::SetCapture { .. } => self.emit_variable_access(op),
            // ── 控制流 ──
            IrOp::Jump { .. } | IrOp::JumpIf { .. } | IrOp::Return { .. } => {
                self.emit_control_flow(op)
            }
            // ── 复合类型创建（含范围构造）──
            IrOp::ArrayNew { .. } | IrOp::ObjectNew { .. } | IrOp::RangeNew { .. } => {
                self.emit_composite_types(op)
            }
            // ── 属性与索引访问 ──
            IrOp::GetField { .. }
            | IrOp::SetField { .. }
            | IrOp::IndexGet { .. }
            | IrOp::IndexSet { .. }
            | IrOp::IndexSetMut { .. } => self.emit_property_access(op),
            // ── 异常处理 ──
            IrOp::TryStart { .. } | IrOp::TryEnd | IrOp::Out { .. } => {
                self.emit_exception_handling(op)
            }
            // ── 调试打印 ──
            IrOp::Print { .. } => self.emit_debug(op),
            // ── 长度查询 ──
            IrOp::Len { .. } => self.emit_len(op),
            // ── 字符串批量拼接 ──
            IrOp::StringBuild { .. } => self.emit_string_build(op),
            // ── 切片链字符串构建器 (SCSB) ──
            IrOp::SliceChainInit { .. }
            | IrOp::SliceChainAppend { .. }
            | IrOp::SliceChainFinish { .. } => self.emit_slicechain(op),
            // ── 条件选择（Phi）──
            IrOp::Select { .. } => self.emit_select(op),
        }
    }

    // ── 发射辅助方法（#[method] 由 nuzo_class 编译期校验 &self/&mut self 签名）──
    //
    // 约定：每个辅助方法只处理一类语义相关的 IrOp 变体。
    // 分发器 emit_op 保证只会传入对应类别的变体，
    // 因此 `_` 通配分支用 unreachable! 标记编程错误（而非运行时错误）。

    /// 字面量加载：LoadConstant / LoadArg / Mov
    fn emit_literal_load(&mut self, op: &IrOp) -> Result<(), CodegenError> {
        match op {
            // ── 字面量加载 ──
            IrOp::LoadConstant { dest, constant } => {
                let dest_reg = self.rm()?.allocate_def(*dest)?;
                let const_idx = self.add_constant(constant)?;
                self.chunk.emit(Instruction::LoadK {
                    dest: Reg(dest_reg),
                    const_idx: ConstIdx(const_idx),
                });
            }

            // ── 参数加载 ──
            // 参数在 VM 约定中位于前 N 个寄存器（r0, r1, ... rN-1）
            // 如果目标寄存器与参数寄存器不同，需要 Mov
            IrOp::LoadArg { dest, index } => {
                let dest_reg = self.rm()?.allocate_def(*dest)?;
                let param_reg = *index;
                if dest_reg != param_reg {
                    self.chunk.emit(Instruction::Mov { dest: Reg(dest_reg), src: Reg(param_reg) });
                }
            }

            // ── 寄存器移动（degenerate Phi: 控制流值合并）──
            // 将 src 寄存器的值复制到 dest 寄存器。
            // 用于 if/and/or 等控制流在多个分支中统一结果值。
            // 当 dest 和 src 映射到同一物理寄存器时省略 Mov 指令。
            IrOp::Mov { dest, src } => {
                let src_reg = self.rm()?.consume_use(*src)?;
                let dest_reg = self.rm()?.allocate_def(*dest)?;
                if dest_reg != src_reg {
                    self.chunk.emit(Instruction::Mov { dest: Reg(dest_reg), src: Reg(src_reg) });
                }
            }

            // A10 修复: 用 UnexpectedIrOp 错误替代 unreachable! panic,
            // 使 IR 层新增变体未同步时能优雅返回错误而非崩溃。
            _ => {
                return Err(CodegenError::UnexpectedIrOp {
                    op: format!("{:?}", op),
                    context: "emit_literal_load",
                });
            }
        }
        Ok(())
    }

    /// 算术运算：Binary / Unary
    fn emit_arithmetic(&mut self, op: &IrOp) -> Result<(), CodegenError> {
        match op {
            // ── 二元运算 ──
            IrOp::Binary { dest, op, left, right } => {
                let left_reg = self.rm()?.consume_use(*left)?;
                let right_reg = self.rm()?.consume_use(*right)?;
                let dest_reg = self.rm()?.allocate_def(*dest)?;
                let instr = self.binary_to_instruction(*op, dest_reg, left_reg, right_reg);
                self.chunk.emit(instr);
            }

            // ── 一元运算 ──
            IrOp::Unary { dest, op, operand } => {
                let operand_reg = self.rm()?.consume_use(*operand)?;
                let dest_reg = self.rm()?.allocate_def(*dest)?;
                match op {
                    IrUnaryOp::Neg => {
                        self.chunk
                            .emit(Instruction::Neg { dest: Reg(dest_reg), src: Reg(operand_reg) });
                    }
                    IrUnaryOp::Not => {
                        self.chunk
                            .emit(Instruction::Not { dest: Reg(dest_reg), src: Reg(operand_reg) });
                    }
                }
            }

            // A10: 替代 unreachable! panic
            _ => {
                return Err(CodegenError::UnexpectedIrOp {
                    op: format!("{:?}", op),
                    context: "emit_arithmetic",
                });
            }
        }
        Ok(())
    }

    /// 函数调用与闭包：Call / Closure
    fn emit_call_closure(&mut self, op: &IrOp) -> Result<(), CodegenError> {
        match op {
            // ── 函数调用 ──
            // VM 约定：callee 在 func_reg，参数在 func_reg+1 .. func_reg+argc
            // 需要将参数排列到 callee 之后的连续寄存器
            IrOp::Call { dest, callee, args } => {
                let callee_reg = self.rm()?.consume_use(*callee)?;

                // 🔧 Fix: VM Call 约定返回值覆盖 func 寄存器 (callee_reg)。
                // 如果 callee ValueRef 后续还会被引用（如循环中重复调用 f(args)），
                // Call 执行后 callee_reg 存的是返回值而非原始 callee → TypeMismatch。
                // 解决：Call 前将 callee 值复制到新临时寄存器，用新寄存器作为真正的 func 参数。
                let actual_callee_reg = {
                    let save_reg = self.rm()?.allocate_temp()?;
                    self.chunk.emit(Instruction::Mov { dest: Reg(save_reg), src: Reg(callee_reg) });
                    save_reg
                };

                // 将参数 Move 到 actual_callee_reg 之后的连续位置
                let argc = Self::narrow_u8(args.len())?;
                let mut arg_regs = Vec::with_capacity(args.len());
                for arg_vr in args.iter() {
                    arg_regs.push(self.rm()?.consume_use(*arg_vr)?);
                }
                for (i, arg_reg) in arg_regs.iter().enumerate() {
                    let target_reg = actual_callee_reg
                        .checked_add(1)
                        .and_then(|r| r.checked_add(i as u16))
                        .ok_or(CodegenError::TooManyRegisters {
                            count: actual_callee_reg as u32 + 1 + i as u32,
                        })?;
                    if *arg_reg != target_reg {
                        self.chunk
                            .emit(Instruction::Mov { dest: Reg(target_reg), src: Reg(*arg_reg) });
                    }
                }

                self.chunk.emit(Instruction::Call { func: Reg(actual_callee_reg), argc: U8(argc) });

                // 返回值在 actual_callee_reg（VM 约定）
                if let Some(dest_vr) = dest {
                    // 返回值在 actual_callee_reg，为 dest 分配该寄存器
                    // 注意：Call 返回值固定在 func 寄存器，不经过 allocate_def
                    // 但我们仍需让 reg_manager 记录这个映射，否则后续 consume_use(dest) 会失败
                    // 使用 allocate_def 让 reg_manager 分配一个寄存器并记录，
                    // 但实际值在 actual_callee_reg，所以需要把 allocate_def 分配的寄存器释放，
                    // 然后将 actual_callee_reg 注册给 dest。
                    // 更好的方案：直接在 reg_manager 中记录 dest → actual_callee_reg 的映射。
                    // 但 RegisterManager trait 不支持这种方式。
                    // 最简单的方案：为 dest 分配一个临时寄存器作为"def"，
                    // 然后 Mov 返回值到 dest 的寄存器。
                    // 但这会引入不必要的 Mov。
                    // 实际上，对于 Call，返回值的寄存器号由 VM 约定决定（= actual_callee_reg），
                    // 不是由 reg_manager 分配。我们只需要让后续 consume_use(dest) 能找到它。
                    // 暂时用 allocate_def 分配，如果分配结果 != actual_callee_reg，
                    // 则需要 Mov。由于 allocate_def 可能复用刚释放的寄存器，
                    // 最常见的情形是 dest_reg == actual_callee_reg。
                    let dest_reg = self.rm()?.allocate_def(*dest_vr)?;
                    if dest_reg != actual_callee_reg {
                        self.chunk.emit(Instruction::Mov {
                            dest: Reg(dest_reg),
                            src: Reg(actual_callee_reg),
                        });
                    }
                }
            }

            // ── 闭包创建 ──
            // 从 closure_indices 映射表查找 IrFunctionId 对应的常量池索引。
            // 常量池中存储的是 HeapObject::Closure（包含 FunctionPrototype），
            // VM 的 Opcode::Closure 从常量池取出 Closure 对象直接写入寄存器。
            IrOp::Closure { dest, ir_func } => {
                let dest_reg = self.rm()?.allocate_def(*dest)?;
                let proto_idx = self.closure_indices.get(&ir_func.0).copied().ok_or_else(|| {
                    CodegenError::Generic {
                        message: format!(
                            "IrOp::Closure 引用了未注册的函数 ID {} \
                             — 函数原型应在 generate() Phase 1 中预生成",
                            ir_func.0
                        ),
                    }
                })?;
                self.chunk
                    .emit(Instruction::Closure { dest: Reg(dest_reg), proto: ConstIdx(proto_idx) });
            }

            // ── 闭包捕获变量填充 ──
            // 在父函数中发射，将捕获变量的值写入闭包的 captured[] 数组
            IrOp::Capture { closure, index, source } => {
                let closure_reg = self.rm()?.consume_use(*closure)?;
                let captured_source = match source {
                    CaptureSource::Register(vr) => {
                        let src_reg = self.rm()?.consume_use(*vr)?;
                        CapturedSource::ByValue(Reg(src_reg))
                    }
                    CaptureSource::OuterCapture(outer_idx) => {
                        CapturedSource::Outer(Self::narrow_u8_from_u32(*outer_idx as u32)?)
                    }
                    CaptureSource::Global(name) => {
                        // 全局变量捕获：先 GetGlobal 加载到临时寄存器，再按值捕获
                        let temp_reg = self.rm()?.allocate_temp()?;
                        let name_idx = self
                            .add_constant(&IrConstant::String(name.as_ref().to_string().into()))?;
                        self.chunk.emit(Instruction::GetGlobal {
                            dest: Reg(temp_reg),
                            name: ConstIdx(name_idx),
                            _iss_gidx: U16(0),
                        });
                        CapturedSource::ByValue(Reg(temp_reg))
                    }
                };
                self.chunk.emit(Instruction::Capture {
                    closure: Reg(closure_reg),
                    idx: CaptureIdx(*index),
                    source: captured_source,
                });
            }

            // A10: 替代 unreachable! panic
            _ => {
                return Err(CodegenError::UnexpectedIrOp {
                    op: format!("{:?}", op),
                    context: "emit_call_closure",
                });
            }
        }
        Ok(())
    }

    /// 变量访问：GetLocal / SetLocal / GetGlobal / SetGlobal / GetCapture / SetCapture
    fn emit_variable_access(&mut self, op: &IrOp) -> Result<(), CodegenError> {
        match op {
            // ── 局部变量访问（跨基本块值传递） ──
            //
            // IR 层的 SetLocal 记录 name → ValueRef 映射，
            // GetLocal 查找映射并发射 Mov 实现值传递。
            // 这使得 for-in 等循环结构能在不同基本块间传递迭代器/索引/长度。
            IrOp::SetLocal { name, value } => {
                // 消费 source ValueRef 的使用
                let val_reg = self.rm()?.consume_use(*value)?;
                if let Some(&local_reg) = self.local_map.get(name) {
                    // 变量已存在：发射 Mov 将新值写回专用寄存器，
                    // 确保循环回跳时循环头部能从同一寄存器读到更新后的值。
                    if local_reg != val_reg {
                        self.chunk
                            .emit(Instruction::Mov { dest: Reg(local_reg), src: Reg(val_reg) });
                    }
                } else {
                    // 首次赋值：分配专用寄存器（永不释放），发射 Mov 存储值
                    let local_reg = self.rm()?.allocate_temp()?;
                    self.chunk.emit(Instruction::Mov { dest: Reg(local_reg), src: Reg(val_reg) });
                    self.local_map.insert(name.clone(), local_reg);
                }
            }
            IrOp::GetLocal { dest, name } => {
                // 从 local_map 查找专用寄存器
                let local_reg = *self.local_map.get(name).ok_or_else(|| CodegenError::Generic {
                    message: format!(
                        "GetLocal: undefined local variable '{}' (dest=v{})",
                        name, dest.0
                    ),
                })?;
                // 为 dest 分配物理寄存器
                let dest_reg = self.rm()?.allocate_def(*dest)?;
                // 发射 Mov 从专用寄存器复制到 dest
                self.chunk.emit(Instruction::Mov { dest: Reg(dest_reg), src: Reg(local_reg) });
            }

            // ── 全局变量 ──
            IrOp::GetGlobal { dest, name } => {
                // lazy import 精确延迟发射：如果 name 匹配某个 lazy import 的
                // resolved_symbols，在 GetGlobal 之前先发射 InitModule。
                // 这确保子模块顶层代码（如 print 副作用）在首次引用时执行，
                // 而非在程序开始时（eager）或结束时（flush）执行。
                self.try_emit_lazy_init_for_symbol(name)?;
                let dest_reg = self.rm()?.allocate_def(*dest)?;
                let name_idx =
                    self.add_constant(&IrConstant::String(name.as_ref().to_string().into()))?;
                // GetGlobal 编码格式: opcode(1) + dest:u16(2) + name_idx:u16(2) + _iss_gidx:u16(2) = 7 字节
                self.chunk.emit(Instruction::GetGlobal {
                    dest: Reg(dest_reg),
                    name: ConstIdx(name_idx),
                    _iss_gidx: U16(0), // ISS 缓存槽位，初始为 0，运行时由 VM 内联缓存填充
                });
            }

            IrOp::SetGlobal { name, value } => {
                let value_reg = self.rm()?.consume_use(*value)?;
                let name_idx =
                    self.add_constant(&IrConstant::String(name.as_ref().to_string().into()))?;
                // SetGlobal 字段顺序: val, name（注意不是 name, val！）
                self.chunk
                    .emit(Instruction::SetGlobal { val: Reg(value_reg), name: ConstIdx(name_idx) });
            }

            // ── 闭包捕获变量 ──
            IrOp::GetCapture { dest, index } => {
                let dest_reg = self.rm()?.allocate_def(*dest)?;
                self.chunk.emit(Instruction::GetCaptured {
                    dest: Reg(dest_reg),
                    idx: CaptureIdx(*index),
                });
            }

            IrOp::SetCapture { index, value } => {
                let value_reg = self.rm()?.consume_use(*value)?;
                self.chunk.emit(Instruction::SetCaptured {
                    idx: CaptureIdx(*index),
                    val: Reg(value_reg),
                });
            }

            // A10: 替代 unreachable! panic
            _ => {
                return Err(CodegenError::UnexpectedIrOp {
                    op: format!("{:?}", op),
                    context: "emit_variable_access",
                });
            }
        }
        Ok(())
    }

    /// 控制流：Jump / JumpIf / Return
    fn emit_control_flow(&mut self, op: &IrOp) -> Result<(), CodegenError> {
        match op {
            // ── 跳转 ──
            // 回填策略：先发射占位(offset=0)，Phase 3 统一用实际块位置回填
            IrOp::Jump { target } => {
                let jmp_pos = self.chunk.len(); // Jmp 指令起始位置
                self.chunk.emit(Instruction::Jmp { offset: Offset(0) }); // 占位
                self.jump_fixups.push(JumpFixup {
                    pos: jmp_pos,
                    target_block: target.0,
                    instr_size: 3,
                });
            }

            // JumpIf: 条件为 falsy 时跳转到 else_target，否则落到 then_target
            // 编码模式: Test(cond, else_offset) + Jmp(then_offset)
            //
            // 控制流:
            //   Test: 如果 cond 为 falsy，IP 跳到 (Test_end + else_offset)
            //   Jmp:  无条件跳到 (Jmp_end + then_offset)
            //
            // 两条指令的偏移都使用回填机制
            IrOp::JumpIf { cond, then_target, else_target } => {
                let cond_reg = self.rm()?.consume_use(*cond)?;

                // Test 指令: opcode(1) + Reg(2) + Offset(2) = 5 字节
                let test_pos = self.chunk.len();
                self.chunk.emit(Instruction::Test {
                    reg: Reg(cond_reg),
                    offset: Offset(0), // 占位
                });
                self.jump_fixups.push(JumpFixup {
                    pos: test_pos,
                    target_block: else_target.0,
                    instr_size: 5,
                });

                // Jmp 指令: opcode(1) + Offset(2) = 3 字节
                let jmp_pos = self.chunk.len();
                self.chunk.emit(Instruction::Jmp { offset: Offset(0) }); // 占位
                self.jump_fixups.push(JumpFixup {
                    pos: jmp_pos,
                    target_block: then_target.0,
                    instr_size: 3,
                });
            }

            // Return: Return 指令需要 val: Reg（非 Option）
            // 对于 void return，先 LoadNil 到临时寄存器
            IrOp::Return { value } => {
                // 顶层 main 函数：发射 Halt 而非 Return。
                // 顶层代码没有调用者帧，pop_frame() 会触发 StackUnderflow。
                // Halt 设置 cx.running = false，run_inner 返回 registers[0]。
                // 关键：必须将返回值 Mov 到 r0，否则 run_inner() 返回的是 r0 中的旧值（可能是 Closure 对象）
                if self.is_main_function {
                    if let Some(vr) = value {
                        let return_reg = self.rm()?.consume_use(*vr)?;
                        if return_reg != 0 {
                            self.chunk
                                .emit(Instruction::Mov { dest: Reg(0), src: Reg(return_reg) });
                        }
                    }
                    self.chunk.emit(Instruction::Halt);
                    return Ok(());
                }
                match value {
                    Some(vr) => {
                        let reg = self.rm()?.consume_use(*vr)?;
                        self.chunk.emit(Instruction::Return { val: Reg(reg) });
                    }
                    None => {
                        // void return → LoadNil 到一个临时寄存器然后 Return
                        let nil_reg = self.rm()?.allocate_temp()?;
                        self.chunk.emit(Instruction::LoadNil { dest: Reg(nil_reg) });
                        self.chunk.emit(Instruction::Return { val: Reg(nil_reg) });
                        self.rm()?.deallocate_temp(nil_reg);
                    }
                }
            }

            // A10: 替代 unreachable! panic
            _ => {
                return Err(CodegenError::UnexpectedIrOp {
                    op: format!("{:?}", op),
                    context: "emit_control_flow",
                });
            }
        }
        Ok(())
    }

    /// 复合类型创建：ArrayNew / ObjectNew
    fn emit_composite_types(&mut self, op: &IrOp) -> Result<(), CodegenError> {
        match op {
            // ── 数组创建 ──
            // 数组创建: ArrayNew(count) + 逐个 SetIndex
            // DualPool 重构: idx_reg 循环外一次分配，分区隔离无需 avoiding；
            // checkpoint/restore 批量管理持久区寄存器。
            IrOp::ArrayNew { dest, elements } => {
                let dest_reg = self.rm()?.allocate_def(*dest)?;
                let checkpoint = self.rm()?.save_checkpoint();
                let idx_reg = self.rm()?.allocate_temp()?; // 循环外一次

                // 消费所有元素（获取其物理寄存器）
                let mut elem_regs = Vec::with_capacity(elements.len());
                for elem_vr in elements.iter() {
                    elem_regs.push(self.rm()?.consume_use(*elem_vr)?);
                }

                // 创建空数组
                self.chunk.emit(Instruction::ArrayNew {
                    dest: Reg(dest_reg),
                    count: U16(Self::narrow_u16(elements.len())?),
                });

                // 逐个设置元素: LoadK idx + SetIndexMut
                // 性能修复: 原使用 SetIndex（COW 语义，每次 clone 整个数组 → O(n²)），
                // 改为 SetIndexMut（原地修改 → O(n)）。数组由 ArrayNew 刚创建、独占引用，
                // 原地修改语义正确且零克隆。
                // DualPool 分区隔离保证 idx_reg 不与 elem_regs 冲突
                for (i, &elem_reg) in elem_regs.iter().enumerate() {
                    let idx_const = self.add_constant(&IrConstant::Number(i as f64))?;
                    self.chunk.emit(Instruction::LoadK {
                        dest: Reg(idx_reg),
                        const_idx: ConstIdx(idx_const),
                    });
                    self.chunk.emit(Instruction::SetIndexMut {
                        obj: Reg(dest_reg),
                        index: Reg(idx_reg),
                        val: Reg(elem_reg),
                    });
                }

                self.rm()?.deallocate_temp(idx_reg);
                self.rm()?.restore_checkpoint(checkpoint);
            }

            // ── 对象创建 ──
            // 字典字面量：创建空字典常量并 LoadK 到目标寄存器，
            // 后续 SetProp 指令会逐个填充键值对（与旧编译器 compile_dict 一致）
            IrOp::ObjectNew { dest } => {
                let dest_reg = self.rm()?.allocate_def(*dest)?;
                let dict_value = Value::from_heap_object_gc(HeapObject::Dict(NuzoDict::new()));
                // C1 增强: 使用 try_add_constant 替代 add_constant,
                // 修复原实现的潜在 u16 截断 bug(原直接 as u16,溢出时截断加载错误常量)。
                let const_idx = self
                    .chunk
                    .try_add_constant(dict_value)
                    .map_err(|_| CodegenError::ConstantPoolOverflow)?;
                self.chunk.emit(Instruction::LoadK {
                    dest: Reg(dest_reg),
                    const_idx: ConstIdx(Self::narrow_u16(const_idx)?),
                });
            }

            // ── 范围构造 ──
            // 创建范围对象 dest = start..end (inclusive 控制闭/半开区间)
            // 对应字节码 Instruction::RangeNew（8 字节）
            IrOp::RangeNew { dest, start, end, inclusive } => {
                let start_reg = self.rm()?.consume_use(*start)?;
                let end_reg = self.rm()?.consume_use(*end)?;
                let dest_reg = self.rm()?.allocate_def(*dest)?;
                self.chunk.emit(Instruction::RangeNew {
                    dest: Reg(dest_reg),
                    start: Reg(start_reg),
                    end: Reg(end_reg),
                    inclusive: U8(if *inclusive { 1 } else { 0 }),
                });
            }

            // A10: 替代 unreachable! panic
            _ => {
                return Err(CodegenError::UnexpectedIrOp {
                    op: format!("{:?}", op),
                    context: "emit_composite_types",
                });
            }
        }
        Ok(())
    }

    /// 属性与索引访问：GetField / SetField / IndexGet / IndexSet
    fn emit_property_access(&mut self, op: &IrOp) -> Result<(), CodegenError> {
        match op {
            // ── 属性访问 ──
            IrOp::GetField { dest, object, field } => {
                let obj_reg = self.rm()?.consume_use(*object)?;
                let dest_reg = self.rm()?.allocate_def(*dest)?;
                let field_idx =
                    self.add_constant(&IrConstant::String(field.as_ref().to_string().into()))?;
                self.chunk.emit(Instruction::GetProp {
                    dest: Reg(dest_reg),
                    obj: Reg(obj_reg),
                    prop: ConstIdx(field_idx),
                });
            }

            IrOp::SetField { object, field, value } => {
                let obj_reg = self.rm()?.consume_use(*object)?;
                let val_reg = self.rm()?.consume_use(*value)?;
                let field_idx =
                    self.add_constant(&IrConstant::String(field.as_ref().to_string().into()))?;
                self.chunk.emit(Instruction::SetProp {
                    obj: Reg(obj_reg),
                    prop: ConstIdx(field_idx),
                    val: Reg(val_reg),
                });
            }

            // ── 索引访问 ──
            IrOp::IndexGet { dest, object, index } => {
                let obj_reg = self.rm()?.consume_use(*object)?;
                let idx_reg = self.rm()?.consume_use(*index)?;
                let dest_reg = self.rm()?.allocate_def(*dest)?;
                self.chunk.emit(Instruction::GetIndex {
                    dest: Reg(dest_reg),
                    obj: Reg(obj_reg),
                    index: Reg(idx_reg),
                });
            }

            IrOp::IndexSet { object, index, value } => {
                let obj_reg = self.rm()?.consume_use(*object)?;
                let idx_reg = self.rm()?.consume_use(*index)?;
                let val_reg = self.rm()?.consume_use(*value)?;
                self.chunk.emit(Instruction::SetIndex {
                    obj: Reg(obj_reg),
                    index: Reg(idx_reg),
                    val: Reg(val_reg),
                });
            }
            IrOp::IndexSetMut { object, index, value } => {
                let obj_reg = self.rm()?.consume_use(*object)?;
                let idx_reg = self.rm()?.consume_use(*index)?;
                let val_reg = self.rm()?.consume_use(*value)?;
                self.chunk.emit(Instruction::SetIndexMut {
                    obj: Reg(obj_reg),
                    index: Reg(idx_reg),
                    val: Reg(val_reg),
                });
            }

            // A10: 替代 unreachable! panic
            _ => {
                return Err(CodegenError::UnexpectedIrOp {
                    op: format!("{:?}", op),
                    context: "emit_property_access",
                });
            }
        }
        Ok(())
    }

    /// 异常处理：TryStart / TryEnd / Out
    fn emit_exception_handling(&mut self, op: &IrOp) -> Result<(), CodegenError> {
        match op {
            // ── Try 块开始 ──
            // TryStart 标记 try 块开始，catch_target 指向 catch 块的基本块 ID。
            // codegen 时将 catch_target 转换为相对偏移 catch_offset。
            // 编码格式: opcode(1) + catch_offset:i16(2) + exception_reg:u8(1) = 4 字节
            IrOp::TryStart { catch_target, exception_reg } => {
                let catch_offset = self.compute_jump_offset(*catch_target, 4)?;
                self.chunk.emit(Instruction::TryStart {
                    catch_offset: Offset(catch_offset),
                    exception_reg: U8(*exception_reg),
                });
            }

            // ── Try 块结束 ──
            // TryEnd 标记 try 块正常结束（无异常抛出时的清理点）。
            // 编码格式: 仅 opcode(1) = 1 字节
            IrOp::TryEnd => {
                self.chunk.emit(Instruction::TryEnd);
            }

            // ── 抛出异常 ──
            // Out 抛出异常：取 value 的值，查找最近的 TryStart，跳转到对应 catch 块。
            // 语义上等价于终止指令（抛出后控制流不再顺序执行）。
            // 编码格式: opcode(1) + value_reg:u16(2) = 3 字节
            IrOp::Out { value } => {
                let value_reg = self.rm()?.consume_use(*value)?;
                self.chunk.emit(Instruction::Out { value_reg: Reg(value_reg) });
            }

            // A10: 替代 unreachable! panic
            _ => {
                return Err(CodegenError::UnexpectedIrOp {
                    op: format!("{:?}", op),
                    context: "emit_exception_handling",
                });
            }
        }
        Ok(())
    }

    /// 调试打印：Print
    fn emit_debug(&mut self, op: &IrOp) -> Result<(), CodegenError> {
        match op {
            // ── 打印 / 调试 ──
            IrOp::Print { value } => {
                let reg = self.rm()?.consume_use(*value)?;
                self.chunk.emit(Instruction::Print { reg: Reg(reg) });
            }
            // A10: 替代 unreachable! panic
            _ => {
                return Err(CodegenError::UnexpectedIrOp {
                    op: format!("{:?}", op),
                    context: "emit_debug",
                });
            }
        }
        Ok(())
    }

    /// 长度查询：Len
    fn emit_len(&mut self, op: &IrOp) -> Result<(), CodegenError> {
        match op {
            IrOp::Len { dest, object } => {
                let object_reg = self.rm()?.consume_use(*object)?;
                let dest_reg = self.rm()?.allocate_def(*dest)?;
                self.chunk.emit(Instruction::Len {
                    dest: Reg(dest_reg),
                    src: Reg(object_reg), // Instruction::Len uses 'src' not 'object'
                });
            }
            // A10: 替代 unreachable! panic
            _ => {
                return Err(CodegenError::UnexpectedIrOp {
                    op: format!("{:?}", op),
                    context: "emit_len",
                });
            }
        }
        Ok(())
    }

    /// 字符串批量拼接：dest = concat(operands[0..N])
    ///
    /// 翻译为 `Instruction::StringBuild { dest, start, count }`。
    ///
    /// # 寄存器布局策略
    ///
    /// StringBuild 要求操作数存放在从 `start` 开始的连续寄存器中。
    /// 由于 reg_manager 的 `allocate_temp` 不保证连续（会从 `temp_free` 复用），
    /// 使用 `allocate_temp_block(count)` 一次性分配 `[start, start+count)` 连续区间：
    ///
    /// 1. 逐个 `consume_use` 每个操作数，收集物理寄存器号（顺带释放已死 temp）
    /// 2. `allocate_def(dest)` 分配结果寄存器
    /// 3. `allocate_temp_block(count)` 分配连续操作数区间 `[start, start+count)`
    /// 4. 将操作数 `Mov` 到 `start, start+1, ..., start+count-1`（已在正确位置则跳过）
    /// 5. 发射 `StringBuild { dest, start, count }`
    /// 6. `deallocate_temp_block(start, count)` 释放操作数区间（StringBuild 后不再需要）
    ///
    /// # 估算一致性
    ///
    /// `estimate_instruction_size` 中 StringBuild 估算为 `7 + N*5`（指令 7 字节 + N 条 Mov），
    /// 与本方法实际发射的字节数一致（最坏情况：每个操作数都需要 Mov）。
    fn emit_string_build(&mut self, op: &IrOp) -> Result<(), CodegenError> {
        match op {
            IrOp::StringBuild { dest, operands } => {
                let count = Self::narrow_u16(operands.len())?;

                // Phase 1: 消费所有操作数的源寄存器（顺带释放已死 temp 回 temp_free）
                let mut src_regs = Vec::with_capacity(operands.len());
                for vr in operands.iter() {
                    src_regs.push(self.rm()?.consume_use(*vr)?);
                }

                // Phase 2: 分配 dest 寄存器（持久区，单调递增）
                let dest_reg = self.rm()?.allocate_def(*dest)?;

                // Phase 3: 分配连续操作数区间 [start, start+count)
                // 必须在 allocate_def 之后，避免 dest_reg 落在区间内被覆盖。
                // allocate_temp_block 从 top 推进 count 个位置，保证物理连续。
                let start_reg = self.rm()?.allocate_temp_block(count)?;

                // Phase 4: 将操作数 Move 到连续位置
                for (i, &src_reg) in src_regs.iter().enumerate() {
                    let target_reg = start_reg + i as u16;
                    if src_reg != target_reg {
                        self.chunk
                            .emit(Instruction::Mov { dest: Reg(target_reg), src: Reg(src_reg) });
                    }
                }

                // Phase 5: 发射 StringBuild 指令
                self.chunk.emit(Instruction::StringBuild {
                    dest: Reg(dest_reg),
                    start: Reg(start_reg),
                    count: U16(count),
                });

                // Phase 6: 释放连续操作数区间（StringBuild 执行后不再需要）
                self.rm()?.deallocate_temp_block(start_reg, count);
            }
            _ => {
                return Err(CodegenError::UnexpectedIrOp {
                    op: format!("{:?}", op),
                    context: "emit_string_build",
                });
            }
        }
        Ok(())
    }

    /// 切片链字符串构建器 (SCSB) 代码生成
    ///
    /// 翻译 3 种 IrOp 为对应字节码：
    /// - `SliceChainInit { dest }` → `SliceChainNew { dest }`
    /// - `SliceChainAppend { chain, src }` → `SliceChainAppend { chain, src }`
    /// - `SliceChainFinish { dest, chain }` → `SliceChainFinish { dest, chain }`
    fn emit_slicechain(&mut self, op: &IrOp) -> Result<(), CodegenError> {
        match op {
            IrOp::SliceChainInit { dest } => {
                let dest_reg = self.rm()?.allocate_def(*dest)?;
                self.chunk.emit(Instruction::SliceChainNew { dest: Reg(dest_reg) });
            }
            IrOp::SliceChainAppend { chain, src } => {
                let src_reg = self.rm()?.consume_use(*src)?;
                let chain_reg = self.rm()?.consume_use(*chain)?;
                self.chunk.emit(Instruction::SliceChainAppend {
                    chain: Reg(chain_reg),
                    src: Reg(src_reg),
                });
            }
            IrOp::SliceChainFinish { dest, chain } => {
                let chain_reg = self.rm()?.consume_use(*chain)?;
                let dest_reg = self.rm()?.allocate_def(*dest)?;
                self.chunk.emit(Instruction::SliceChainFinish {
                    dest: Reg(dest_reg),
                    chain: Reg(chain_reg),
                });
            }
            _ => {
                return Err(CodegenError::UnexpectedIrOp {
                    op: format!("{:?}", op),
                    context: "emit_slicechain",
                });
            }
        }
        Ok(())
    }

    /// 条件选择（Poor man's Phi）：dest = condition ? then_value : else_value
    ///
    /// 翻译为字节码序列：
    ///   Test(condition) → +8  (跳过 Mov+Jmp，到达 else 分支的 Mov)
    ///   Mov(dest, then_value)
    ///   Jmp → +5          (跳过 else 分支的 Mov，到达结束)
    ///   Mov(dest, else_value)
    ///
    /// 所有偏移在编译期已知（同基本块内的前向跳转），无需回填。
    fn emit_select(&mut self, op: &IrOp) -> Result<(), CodegenError> {
        match op {
            IrOp::Select { condition, then_value, else_value, dest } => {
                let cond_reg = self.rm()?.consume_use(*condition)?;
                let then_reg = self.rm()?.consume_use(*then_value)?;
                let else_reg = self.rm()?.consume_use(*else_value)?;
                let dest_reg = self.rm()?.allocate_def(*dest)?;

                // Test: 条件为 falsy 时跳到 else Mov（跳过 Mov(then) + Jmp = 8 字节）
                self.chunk.emit(Instruction::Test {
                    reg: Reg(cond_reg),
                    offset: Offset(8), // Mov(5) + Jmp(3)
                });
                // Then 分支：dest = then_value
                self.chunk.emit(Instruction::Mov { dest: Reg(dest_reg), src: Reg(then_reg) });
                // 跳过 else 分支的 Mov（5 字节）
                self.chunk.emit(Instruction::Jmp { offset: Offset(5) });
                // Else 分支：dest = else_value
                self.chunk.emit(Instruction::Mov { dest: Reg(dest_reg), src: Reg(else_reg) });
            }
            // A10: 替代 unreachable! panic
            _ => {
                return Err(CodegenError::UnexpectedIrOp {
                    op: format!("{:?}", op),
                    context: "emit_select",
                });
            }
        }
        Ok(())
    }

    // ── 辅助方法：寄存器管理 ──

    /// 获取当前 RegisterManager 的可变引用
    ///
    /// 必须在 `generate_function()` 之后调用（此时 reg_manager 已构建），
    /// 否则返回 "RegisterManager not initialized" 错误。
    fn rm(&mut self) -> Result<&mut (dyn RegisterManager + '_), CodegenError> {
        self.reg_manager.as_mut().map(|b| b.as_mut() as &mut dyn RegisterManager).ok_or_else(|| {
            CodegenError::Generic { message: "RegisterManager not initialized".into() }
        })
    }

    // ── 辅助方法：常量池管理 ──

    // ── 辅助方法：安全索引窄化 ──

    /// 将 usize 安全窄化为 u16（常量池索引、寄存器索引等）。
    ///
    /// 使用 `SafeIndex::<u16>` 内部检查，溢出时映射为
    /// `CodegenError::ConstantPoolOverflow`。
    fn narrow_u16(val: usize) -> Result<u16, CodegenError> {
        SafeIndex::<u16>::try_from_usize(val)
            .map(|idx| idx.get())
            .map_err(|_| CodegenError::ConstantPoolOverflow)
    }

    /// 将 usize 安全窄化为 u8（参数计数 argc、捕获索引等）。
    ///
    /// 使用 `SafeIndex::<u8>` 内部检查，溢出时映射为
    /// `CodegenError::Generic`（附带诊断信息）。
    fn narrow_u8(val: usize) -> Result<u8, CodegenError> {
        SafeIndex::<u8>::try_from_usize(val).map(|idx| idx.get()).map_err(|e| {
            CodegenError::Generic {
                message: format!("index overflow: {} exceeds u8 range", e.value),
            }
        })
    }

    /// 将 u32 安全窄化为 u8（捕获索引等）。
    ///
    /// 使用 `SafeIndex::<u8>` 内部检查，溢出时映射为
    /// `CodegenError::Generic`（附带诊断信息）。
    fn narrow_u8_from_u32(val: u32) -> Result<u8, CodegenError> {
        SafeIndex::<u8>::try_from_u32(val).map(|idx| idx.get()).map_err(|e| CodegenError::Generic {
            message: format!("index overflow: {} exceeds u8 range", e.value),
        })
    }

    /// 将 IrConstant 转换为 nuzo_core::Value 并添加到常量池
    ///
    /// 返回常量池索引（u16），失败时返回 ConstantPoolOverflow。
    fn add_constant(&mut self, constant: &IrConstant) -> Result<u16, CodegenError> {
        let value = match constant {
            IrConstant::Number(n) => Value::from_number(*n),
            IrConstant::String(s) => Value::from_string(s),
            IrConstant::Bool(b) => Value::from_bool(*b),
            IrConstant::Nil => NIL,
        };
        // C1 增强: 使用 try_add_constant (Result API) 替代 add_constant + 手动检查,
        // 消除死代码(原 add_constant 会在溢出时 panic,后续检查永不触发)。
        let idx =
            self.chunk.try_add_constant(value).map_err(|_| CodegenError::ConstantPoolOverflow)?;
        Self::narrow_u16(idx)
    }

    // ── 辅助方法：跳转偏移计算 ──

    /// 计算从当前跳转指令到目标基本块的相对偏移
    ///
    /// # Offset 语义
    /// VM 的 Offset 是相对于**下一条指令**首字节的字节偏移。
    /// 即：执行跳转后，IP = (当前指令末尾) + offset
    ///
    /// # 公式
    /// ```text
    /// offset = target_start - (current_pos + instr_size)
    ///          ^^^^^^^^^^^^^   ^^^^^^^^^^^^^^^^^^^^^^^
    ///          目标块绝对偏移   下一条指令的绝对位置
    /// ```
    /// 其中：
    /// - `target_start`: 预扫描计算的目标块起始字节偏移
    /// - `current_pos`: 当前跳转指令的起始位置（`chunk.len()` 发射前）
    /// - `instr_size`: 当前跳转指令的字节大小（Jmp=3, Test=5）
    ///
    /// # 参数
    /// * `target`: 目标基本块 ID
    /// * `instr_size`: 当前正在发射的跳转指令的字节大小
    ///
    /// # 错误
    /// * `InvalidJumpTarget`: 目标块不在 `block_starts` 中（预扫描遗漏）
    /// * `JumpOffsetOverflow`: 偏移超出 i16 范围（±32767 字节，约 32KB）
    fn compute_jump_offset(
        &self,
        target: BasicBlockId,
        instr_size: usize,
    ) -> Result<i16, CodegenError> {
        let target_pos = self
            .block_starts
            .get(&target.0)
            .copied()
            .ok_or(CodegenError::InvalidJumpTarget { target })?;

        let current_pos = self.chunk.len();
        // 下一条指令的位置 = 当前指令位置 + 当前指令大小
        let next_instr_pos = current_pos + instr_size;
        let raw_offset = target_pos as i64 - next_instr_pos as i64;

        i16::try_from(raw_offset)
            .map_err(|_| CodegenError::JumpOffsetOverflow { offset: raw_offset })
    }

    /// Phase 3: 回填所有跳转偏移
    ///
    /// 遍历 jump_fixups 列表，用 Phase 2 记录的实际块起始位置计算正确偏移，
    /// 然后修改 chunk 中对应位置的 offset 字段。
    fn resolve_jump_fixups(&mut self) -> Result<(), CodegenError> {
        for fixup in &self.jump_fixups {
            let target_pos = self.block_starts.get(&fixup.target_block).copied().ok_or(
                CodegenError::InvalidJumpTarget { target: BasicBlockId(fixup.target_block) },
            )?;

            // 下一条指令位置 = 跳转指令位置 + 指令大小
            let next_instr_pos = fixup.pos + fixup.instr_size;
            let raw_offset = target_pos as i64 - next_instr_pos as i64;
            let offset = i16::try_from(raw_offset)
                .map_err(|_| CodegenError::JumpOffsetOverflow { offset: raw_offset })?;

            // 将偏移值写入 chunk 的字节码中
            // offset 字段在操作数区，位置 = pos + 1(opcode) + 前面操作数
            // 对于 Jmp(3): [opcode, offset_lo, offset_hi] → offset 在 [pos+1, pos+2]
            // 对于 Test(5): [opcode, reg_lo, reg_hi, offset_lo, offset_hi] → offset 在 [pos+3, pos+4]
            let code = self.chunk.code_mut();
            let offset_bytes = offset.to_le_bytes();
            // H4 修复: 写入前检查 fixup.pos + instr_size 是否在 code 边界内。
            // 防止 Phase 2 发射与回填记录不一致(内部 bug 或 IR 损坏)时
            // 索引越界导致 panic,改为返回 FixupOutOfBounds 错误。
            let end = fixup.pos + fixup.instr_size;
            if end > code.len() {
                return Err(CodegenError::FixupOutOfBounds {
                    pos: fixup.pos,
                    instr_size: fixup.instr_size,
                    code_len: code.len(),
                });
            }
            match fixup.instr_size {
                3 => {
                    // Jmp: offset at [pos+1, pos+2]
                    code[fixup.pos + 1] = offset_bytes[0];
                    code[fixup.pos + 2] = offset_bytes[1];
                }
                5 => {
                    // Test: offset at [pos+3, pos+4]
                    code[fixup.pos + 3] = offset_bytes[0];
                    code[fixup.pos + 4] = offset_bytes[1];
                }
                _ => {
                    return Err(CodegenError::Generic {
                        message: format!(
                            "unexpected instr_size {} in jump fixup",
                            fixup.instr_size
                        ),
                    });
                }
            }
        }

        Ok(())
    }

    // ── 辅助方法：二元运算映射 ──

    /// 将 IrBinOp 映射为对应的 Instruction 变体
    #[inline]
    fn binary_to_instruction(&self, op: IrBinOp, dest: u16, left: u16, right: u16) -> Instruction {
        match op {
            IrBinOp::Add => {
                Instruction::Add { dest: Reg(dest), left: Reg(left), right: Reg(right) }
            }
            IrBinOp::Sub => {
                Instruction::Sub { dest: Reg(dest), left: Reg(left), right: Reg(right) }
            }
            IrBinOp::Mul => {
                Instruction::Mul { dest: Reg(dest), left: Reg(left), right: Reg(right) }
            }
            IrBinOp::Div => {
                Instruction::Div { dest: Reg(dest), left: Reg(left), right: Reg(right) }
            }
            IrBinOp::Mod => {
                Instruction::Mod { dest: Reg(dest), left: Reg(left), right: Reg(right) }
            }
            IrBinOp::Pow => Instruction::Pow { dest: Reg(dest), base: Reg(left), exp: Reg(right) },
            IrBinOp::Eq => Instruction::Eq { dest: Reg(dest), left: Reg(left), right: Reg(right) },
            IrBinOp::Neq => {
                Instruction::Neq { dest: Reg(dest), left: Reg(left), right: Reg(right) }
            }
            IrBinOp::Lt => Instruction::Lt { dest: Reg(dest), left: Reg(left), right: Reg(right) },
            IrBinOp::Gt => Instruction::Gt { dest: Reg(dest), left: Reg(left), right: Reg(right) },
            IrBinOp::Le => Instruction::Le { dest: Reg(dest), left: Reg(left), right: Reg(right) },
            IrBinOp::Ge => Instruction::Ge { dest: Reg(dest), left: Reg(left), right: Reg(right) },
        }
    }

    // ── LSRA 后处理 ──

    /// 运行线性扫描寄存器分配（LSRA）后处理
    ///
    /// 在 `generate()` 发射完所有字节码后调用。基于 `def_ips`/`use_ips` 构建活跃区间，
    /// 交给 `LsraAllocator` 分配物理寄存器，对不重叠区间复用寄存器号以降低 `locals_count`。
    ///
    /// # 策略
    /// - **无 spill**：就地重写 `chunk.code` 中的 Reg 操作数（old_reg → new_phys_reg），
    ///   更新 `locals_count` 为峰值物理寄存器号 + 1。
    /// - **有 spill**：当前阶段回退到原始线性分配（不重映射、不插桩），保证正确性。
    ///   spill 插桩（SpillLoad/SpillStore + 跳转偏移修正）留作后续增强。
    ///
    /// # 错误
    /// - `CodegenError::LsraFailed`：区间构建或分配阶段出错
    #[allow(dead_code)]
    fn apply_lsra(&mut self) -> Result<(), CodegenError> {
        // 空模块或无寄存器分配：直接设置 locals_count 并返回
        if self.chunk.locals_count == 0 {
            self.chunk.locals_count = 0;
            self.chunk.spill_slot_count = 0;
            return Ok(());
        }

        // 1. 构建活跃区间
        let intervals = build_intervals(&self.def_ips, &self.use_ips)
            .map_err(|e| CodegenError::LsraFailed { reason: e.to_string() })?;

        // 无活跃区间（所有寄存器均未同时出现 def+use）→ 回退
        if intervals.is_empty() {
            // locals_count 已由 reg_manager.finalize() 设置，保持不变
            self.chunk.spill_slot_count = 0;
            return Ok(());
        }

        let mut intervals = intervals;

        // 2. NUD 增强（循环深度暂不推导，全 0；租约保护启用）
        let nud_config = NudConfig::default();
        let loop_depths = [0u8; MAX_FUNCTION_LOCALS as usize];
        enhance_intervals(&mut intervals, &nud_config, &loop_depths);

        // 3. 线性扫描分配
        // 使用与 enhance_intervals 相同的 NudConfig，确保租约递减逻辑一致
        let mut lsra = LsraAllocator::with_nud_config(nud_config);
        lsra.allocate(&mut intervals)
            .map_err(|e| CodegenError::LsraFailed { reason: e.to_string() })?;

        // 4. 检测 spill
        let has_spills = intervals.iter().any(|iv| iv.reg.is_none());
        if has_spills {
            self.emit_spill_code(&intervals, &lsra)?;
        } else {
            // 5. 无 spill → 就地重写 Reg 引用
            self.rewrite_regs_with_lsra(&lsra)?;
        }

        Ok(())
    }

    /// 发射 SpillLoad/SpillStore 指令并修正跳转偏移（LSRA spill 路径）
    ///
    /// 当物理寄存器耗尽时，将溢出值存储到栈帧预留的 spill 槽位，
    /// 后续通过 SpillLoad 重新加载。采用两遍扫描策略：
    ///
    /// 1. **第一遍**：扫描原始字节码，构建新字节码缓冲区。在每个指令位置：
    ///    - 如果任何 spilled 区间在此 IP 有 use，先发射 SpillLoad
    ///    - 发射原始指令（寄存器重映射后）
    ///    - 如果任何 spilled 区间在此 IP 有 def，后发射 SpillStore
    ///    - 记录 old_ip → new_ip 映射
    /// 2. **第二遍**：扫描新字节码，用 old_ip → new_ip 映射修正所有跳转偏移
    ///
    /// # 参数
    /// * `intervals` - LSRA 分配后的区间列表（含 spill 信息）
    /// * `lsra` - 已完成 `allocate()` 的分配器实例
    #[allow(dead_code)]
    fn emit_spill_code(
        &mut self,
        intervals: &[crate::allocator::Interval],
        lsra: &LsraAllocator,
    ) -> Result<(), CodegenError> {
        let locals_count = self.chunk.locals_count;

        // 1. 构建重映射表
        //    - 非 spilled: vreg → preg (LSRA 分配结果)
        //    - spilled: vreg → 新分配的物理寄存器（从 max_phys + 1 开始）
        let mut remap: HashMap<u16, u16> = HashMap::new();
        let mut max_phys: u16 = 0;
        for reg in 0..locals_count {
            if let Some(preg) = lsra.get_phys_reg(reg) {
                remap.insert(reg, preg);
                if preg > max_phys {
                    max_phys = preg;
                }
            }
        }

        // 为 spilled 区间分配专用寄存器（从 max_phys + 1 开始，避免冲突）
        let mut next_spill_reg = max_phys.saturating_add(1);
        let mut spill_map: HashMap<u16, (u16, u16)> = HashMap::new();
        // spill_map: vreg → (spill_slot, assigned_reg)
        for iv in intervals {
            if iv.reg.is_none()
                && let Some(slot) = iv.spill_slot
            {
                let assigned_reg = next_spill_reg;
                next_spill_reg = next_spill_reg.saturating_add(1);
                remap.insert(iv.vreg, assigned_reg);
                spill_map.insert(iv.vreg, (slot, assigned_reg));
            }
        }

        // 2. 收集 spill 插入点
        //    def_spills: 按 (def_ip + instr_size) 分组 → 需要在此 IP 之后插入 SpillStore
        //    use_spills: 按 use_ip 分组 → 需要在此 IP 之前插入 SpillLoad
        let mut def_spills: HashMap<usize, Vec<(u16, u16)>> = HashMap::new();
        // key: IP after def instruction, value: Vec<(assigned_reg, spill_slot)>
        let mut use_spills: HashMap<usize, Vec<(u16, u16)>> = HashMap::new();
        // key: IP of use instruction, value: Vec<(assigned_reg, spill_slot)>

        for iv in intervals {
            if iv.reg.is_none()
                && let Some((slot, assigned_reg)) = spill_map.get(&iv.vreg).copied()
            {
                // def point: SpillStore after the instruction
                let def_ip_after = iv.start; // start is the def IP; we insert after the instruction
                def_spills.entry(def_ip_after).or_default().push((assigned_reg, slot));

                // use point: SpillLoad before the instruction
                let use_ip = iv.end; // end is the last use IP
                use_spills.entry(use_ip).or_default().push((assigned_reg, slot));
            }
        }

        // 3. 第一遍：构建新字节码缓冲区
        let old_code = self.chunk.code().to_vec();
        let mut new_code: Vec<u8> = Vec::with_capacity(old_code.len() + spill_map.len() * 10);
        let mut ip_map: HashMap<usize, usize> = HashMap::new(); // old_ip → new_ip

        let mut ip: usize = 0;
        while ip < old_code.len() {
            let opcode_byte = old_code[ip];
            let opcode = match Opcode::decode_opcode(opcode_byte) {
                Some(op) => op,
                None => {
                    return Err(CodegenError::Generic {
                        message: format!(
                            "Spill emit: invalid opcode 0x{:02X} at IP {}",
                            opcode_byte, ip
                        ),
                    });
                }
            };
            let instr_size = opcode.instruction_size();

            // 记录 old_ip → new_ip 映射（指令起始位置）
            ip_map.insert(ip, new_code.len());

            // 3a. 在此 IP 之前插入 SpillLoad（如果任何 spilled 区间在此有 use）
            if let Some(loads) = use_spills.get(&ip) {
                for &(assigned_reg, spill_slot) in loads {
                    // SpillLoad: opcode(54) + reg:u16(2) + slot:u16(2) = 5 bytes
                    new_code.push(Opcode::SpillLoad as u8);
                    new_code.extend_from_slice(&assigned_reg.to_le_bytes());
                    new_code.extend_from_slice(&spill_slot.to_le_bytes());
                }
            }

            // 3b. 发射原始指令（寄存器重映射）
            if ip + instr_size > old_code.len() {
                // 指令被截断，直接复制剩余字节
                new_code.extend_from_slice(&old_code[ip..]);
                ip = old_code.len();
                continue;
            }

            let operands = opcode.operands();
            let operands_sum: usize = operands.iter().map(|k| k.byte_size()).sum();

            // 复制 opcode 字节
            new_code.push(opcode_byte);

            // 处理操作数
            if 1 + operands_sum == instr_size {
                let mut operand_offset = ip + 1;
                for kind in operands {
                    match kind {
                        OperandKind::Reg => {
                            if operand_offset + 1 < old_code.len() {
                                let old_reg = u16::from_le_bytes([
                                    old_code[operand_offset],
                                    old_code[operand_offset + 1],
                                ]);
                                let new_reg = remap.get(&old_reg).copied().unwrap_or(old_reg);
                                new_code.extend_from_slice(&new_reg.to_le_bytes());
                            } else {
                                new_code.push(old_code[operand_offset]);
                                if operand_offset + 1 < old_code.len() {
                                    new_code.push(old_code[operand_offset + 1]);
                                }
                            }
                        }
                        _ => {
                            // 复制操作数字节
                            let end = (operand_offset + kind.byte_size()).min(old_code.len());
                            new_code.extend_from_slice(&old_code[operand_offset..end]);
                        }
                    }
                    operand_offset += kind.byte_size();
                }
            } else {
                // 变长编码指令（如 Capture），直接复制操作数字节
                new_code
                    .extend_from_slice(&old_code[ip + 1..(ip + instr_size).min(old_code.len())]);
            }

            // 3c. 在此 IP 之后插入 SpillStore（如果任何 spilled 区间在此有 def）
            if let Some(stores) = def_spills.get(&ip) {
                for &(assigned_reg, spill_slot) in stores {
                    // SpillStore: opcode(55) + reg:u16(2) + slot:u16(2) = 5 bytes
                    new_code.push(Opcode::SpillStore as u8);
                    new_code.extend_from_slice(&assigned_reg.to_le_bytes());
                    new_code.extend_from_slice(&spill_slot.to_le_bytes());
                }
            }

            ip += instr_size;
        }

        // 4. 第二遍：修正跳转偏移
        //    跳转指令列表：Jmp(code=18,3B), Test(code=19,5B), Try(code=51,4B)
        let mut fix_ip: usize = 0;
        while fix_ip < new_code.len() {
            let opcode_byte = new_code[fix_ip];
            let opcode = match Opcode::decode_opcode(opcode_byte) {
                Some(op) => op,
                None => {
                    fix_ip += 1;
                    continue;
                }
            };
            let instr_size = opcode.instruction_size();
            if fix_ip + instr_size > new_code.len() {
                break;
            }

            // 检查是否需要修正偏移
            let offset_pos = match opcode_byte {
                b if b == Opcode::Jmp as u8 => {
                    // Jmp: opcode(1) + offset:i16(2) = 3 bytes
                    Some(fix_ip + 1)
                }
                b if b == Opcode::Test as u8 => {
                    // Test: opcode(1) + reg:u16(2) + offset:i16(2) = 5 bytes
                    Some(fix_ip + 3)
                }
                b if b == Opcode::TryStart as u8 => {
                    // Try: opcode(1) + offset:i16(2) + exception_reg:u8(1) = 4 bytes
                    Some(fix_ip + 1)
                }
                _ => None,
            };

            if let Some(offset_pos) = offset_pos
                && offset_pos + 2 <= new_code.len()
            {
                let old_offset =
                    i16::from_le_bytes([new_code[offset_pos], new_code[offset_pos + 1]]);
                // 跳转偏移相对于下一条指令起始位置
                let old_target = fix_ip as isize + instr_size as isize + old_offset as isize;

                // 需要找到 old_target 对应的 new_ip
                // 搜索 ip_map 中 <= old_target 的最大 key
                let mut new_target: Option<usize> = None;
                for (&old_ip, &new_ip) in &ip_map {
                    if old_ip as isize <= old_target {
                        new_target = Some(new_ip + (old_target as usize - old_ip));
                    }
                }

                if let Some(target) = new_target {
                    let new_offset = target as isize - fix_ip as isize - instr_size as isize;
                    // 检查偏移是否在 i16 范围内
                    if new_offset >= i16::MIN as isize && new_offset <= i16::MAX as isize {
                        let offset_bytes = (new_offset as i16).to_le_bytes();
                        new_code[offset_pos] = offset_bytes[0];
                        new_code[offset_pos + 1] = offset_bytes[1];
                    }
                }
            }

            fix_ip += instr_size;
        }

        // 5. 替换 chunk 代码并更新元数据
        let new_locals = if remap.is_empty() { 0 } else { next_spill_reg };
        let spill_count = lsra.spill_slot_count();

        let code_vec = self.chunk.code_mut();
        *code_vec = new_code;
        self.chunk.locals_count = new_locals;
        self.chunk.spill_slot_count = spill_count;

        Ok(())
    }

    /// 就地重写 chunk.code 中的 Reg 操作数（LSRA 无 spill 路径）
    ///
    /// 遍历字节码，按 Opcode operands 布局定位每个 `OperandKind::Reg` 字段，
    /// 将 old_reg 替换为 LSRA 分配的 new_phys_reg。不改变字节码长度，
    /// 因此跳转偏移（Offset 操作数）无需修正。
    ///
    /// # 参数
    /// * `lsra` - 已完成 `allocate()` 的分配器实例
    ///
    /// # 错误
    /// * `CodegenError::Generic`：遇到无法解码的 opcode 字节
    #[allow(dead_code)]
    fn rewrite_regs_with_lsra(&mut self, lsra: &LsraAllocator) -> Result<(), CodegenError> {
        // 1. 构建 old_reg → new_phys_reg 重映射表
        //    遍历 0..locals_count 覆盖所有已分配寄存器
        let locals_count = self.chunk.locals_count;
        let mut remap: HashMap<u16, u16> = HashMap::new();
        let mut max_phys: u16 = 0;
        for reg in 0..locals_count {
            if let Some(preg) = lsra.get_phys_reg(reg) {
                remap.insert(reg, preg);
                if preg > max_phys {
                    max_phys = preg;
                }
            }
        }

        // 2. 就地重写 chunk.code 中的 Reg 操作数
        //
        //    遍历策略：
        //    - 用 `Opcode::instruction_size()` 权威前进（含 ISS pad 等额外字节）
        //    - 用 `decode_operand_fields()` 显式定位每个字段的 (kind, offset)
        //      （仅在 operands 求和与 instruction_size 一致时才信任 operands
        //      布局，避免 Capture 等变长编码指令的 tag 字节导致错位）
        //    - 防御性范围检查 `is_remappable_reg(old_reg, locals_count)`：
        //      即使 `Opcode::operands()` 把 ConstIdx 误标为 Reg，常量池索引
        //      通常远大于 locals_count，会被范围检查跳过，避免误改破坏常量池。
        //      （P2.4 TODO 已缓解，完整修复仍需跨 crate 审计 operands() 实现）
        let code = self.chunk.code_mut();
        let mut ip: usize = 0;
        while ip < code.len() {
            let opcode_byte = code[ip];
            let opcode = match Opcode::decode_opcode(opcode_byte) {
                Some(op) => op,
                None => {
                    return Err(CodegenError::Generic {
                        message: format!(
                            "LSRA rewrite: invalid opcode 0x{:02X} at IP {}",
                            opcode_byte, ip
                        ),
                    });
                }
            };

            let instr_size = opcode.instruction_size();

            // 仅当 operands 布局与 instruction_size 完全一致时才重写 Reg 字段
            // （1 + operands_sum == instr_size）。对于 Capture 等变长编码指令，
            // operands 不含 tag 字节，求和 < instruction_size，此时
            // `decode_operand_fields` 返回空 Vec，跳过重写，保留原始 reg 号
            // （安全但次优）。
            let fields = decode_operand_fields(opcode);
            for field in fields {
                if field.kind == OperandKind::Reg {
                    let operand_offset = ip + field.offset;
                    if operand_offset + 1 >= code.len() {
                        break;
                    }
                    // 读取小端 u16 寄存器号
                    let old_reg =
                        (code[operand_offset] as u16) | ((code[operand_offset + 1] as u16) << 8);
                    // 防御性范围检查：仅重写 [0, locals_count) 范围内的值，
                    // 避免 ConstIdx 等被误标为 Reg 的字段被破坏
                    if !is_remappable_reg(old_reg, locals_count) {
                        continue;
                    }
                    if let Some(&new_reg) = remap.get(&old_reg) {
                        // 就地写入新寄存器号（小端）
                        code[operand_offset] = (new_reg & 0xFF) as u8;
                        code[operand_offset + 1] = ((new_reg >> 8) & 0xFF) as u8;
                    }
                }
            }

            // 用 instruction_size 权威前进到下一条指令
            ip += instr_size;
        }

        // 3. 更新 locals_count（峰值物理寄存器号 + 1）和 spill_slot_count
        self.chunk.locals_count = if remap.is_empty() { 0 } else { max_phys + 1 };
        self.chunk.spill_slot_count = lsra.spill_slot_count();

        Ok(())
    }
}

impl Default for CodeGenerator {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// 测试套件
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use nuzo_ir::types::ImportRecord;
    use nuzo_ir::{IrFunctionId, ValueRef};
    use std::path::PathBuf;
    use std::sync::Arc;

    /// 辅助函数：创建一个包含简单常量加载 + 打印的 IrModule
    ///
    /// 代码放在 main 函数（索引 0）中，因为 generate() 只为 main 函数
    /// 生成字节码到顶层 chunk，非 main 函数的字节码在独立的 FunctionPrototype 中。
    fn make_simple_module() -> IrModule {
        let mut module = IrModule::new();
        module.add_function("main");
        let func = module.current_function_mut();
        let block = func.current_block_mut();

        // v0 = load_constant(42.0)
        block.push(IrOp::LoadConstant { dest: ValueRef(0), constant: IrConstant::Number(42.0) });

        // print(v0)
        block.push(IrOp::Print { value: ValueRef(0) });

        // return v0
        block.push(IrOp::Return { value: Some(ValueRef(0)) });

        module
    }

    #[test]
    fn test_codegen_produces_valid_chunk() {
        let mut codegen = CodeGenerator::new();
        let module = make_simple_module();
        let result = codegen.generate(&module);

        assert!(result.is_ok(), "Codegen 应成功: {:?}", result.err());
        let chunk = result.unwrap();

        // main 函数的 Return 映射为 Halt，字节码: LoadK(5) + Print(3) + Halt(1) = 9 字节
        assert!(chunk.len() >= 9, "期望至少 9 字节，实际 {}", chunk.len());

        // 常量池应包含 42.0
        assert!(!chunk.constants().is_empty(), "常量池不应为空");
    }

    #[test]
    fn test_codegen_arithmetic() {
        let mut module = IrModule::new();
        module.add_function("arith");
        let func = module.current_function_mut();
        let block = func.current_block_mut();

        // v0 = 10, v1 = 20, v2 = v0 + v1
        block.push(IrOp::LoadConstant { dest: ValueRef(0), constant: IrConstant::Number(10.0) });
        block.push(IrOp::LoadConstant { dest: ValueRef(1), constant: IrConstant::Number(20.0) });
        block.push(IrOp::Binary {
            dest: ValueRef(2),
            op: IrBinOp::Add,
            left: ValueRef(0),
            right: ValueRef(1),
        });
        block.push(IrOp::Return { value: Some(ValueRef(2)) });

        let mut codegen = CodeGenerator::new();
        let chunk = codegen.generate(&module).expect("算术代码生成成功");

        // 应有 3 个常量: 10.0, 20.0
        assert!(chunk.constants().len() >= 2);
    }

    #[test]
    fn test_codegen_comparison_ops() {
        let ops = [IrBinOp::Eq, IrBinOp::Neq, IrBinOp::Lt, IrBinOp::Gt, IrBinOp::Le, IrBinOp::Ge];

        for &op in &ops {
            let mut module = IrModule::new();
            module.add_function("cmp");
            let func = module.current_function_mut();
            let block = func.current_block_mut();

            block.push(IrOp::LoadConstant { dest: ValueRef(0), constant: IrConstant::Number(1.0) });
            block.push(IrOp::LoadConstant { dest: ValueRef(1), constant: IrConstant::Number(2.0) });
            block.push(IrOp::Binary {
                dest: ValueRef(2),
                op,
                left: ValueRef(0),
                right: ValueRef(1),
            });
            block.push(IrOp::Return { value: Some(ValueRef(2)) });

            let mut codegen = CodeGenerator::new();
            let result = codegen.generate(&module);
            assert!(result.is_ok(), "{:?} 代码生成应成功: {:?}", op, result.err());
        }
    }

    #[test]
    fn test_codegen_unary_ops() {
        for &(op, name) in &[(IrUnaryOp::Neg, "neg"), (IrUnaryOp::Not, "not")] {
            let mut module = IrModule::new();
            module.add_function("unary");
            let func = module.current_function_mut();
            let block = func.current_block_mut();

            block.push(IrOp::LoadConstant { dest: ValueRef(0), constant: IrConstant::Number(5.0) });
            block.push(IrOp::Unary { dest: ValueRef(1), op, operand: ValueRef(0) });
            block.push(IrOp::Return { value: Some(ValueRef(1)) });

            let mut codegen = CodeGenerator::new();
            let result = codegen.generate(&module);
            assert!(result.is_ok(), "{} 代码生成应成功: {:?}", name, result.err());
        }
    }

    #[test]
    fn test_codegen_global_variable() {
        let mut module = IrModule::new();
        module.add_function("global_test");
        let func = module.current_function_mut();
        let block = func.current_block_mut();

        // v0 = get_global("x")
        block.push(IrOp::GetGlobal { dest: ValueRef(0), name: Arc::from("x" as &str) });
        // set_global("x", v0)
        block.push(IrOp::SetGlobal { name: Arc::from("x" as &str), value: ValueRef(0) });
        block.push(IrOp::Return { value: Some(ValueRef(0)) });

        let mut codegen = CodeGenerator::new();
        let result = codegen.generate(&module);
        assert!(result.is_ok(), "全局变量代码生成应成功: {:?}", result.err());
    }

    #[test]
    fn test_codegen_constants_deduplication() {
        let mut module = IrModule::new();
        module.add_function("dedup");
        let func = module.current_function_mut();
        let block = func.current_block_mut();

        // 两次加载相同常量 42.0 → 应只占常量池 1 个槽位（去重）
        block.push(IrOp::LoadConstant { dest: ValueRef(0), constant: IrConstant::Number(42.0) });
        block.push(IrOp::LoadConstant { dest: ValueRef(1), constant: IrConstant::Number(42.0) });
        block.push(IrOp::Return { value: Some(ValueRef(0)) });

        let mut codegen = CodeGenerator::new();
        let chunk = codegen.generate(&module).expect("去重测试成功");

        // 常量池去重: 42.0 只有一个条目
        assert_eq!(chunk.constants().len(), 1, "相同常量应去重");
    }

    #[test]
    fn test_codegen_string_constant() {
        let mut module = IrModule::new();
        module.add_function("str");
        let func = module.current_function_mut();
        let block = func.current_block_mut();

        block.push(IrOp::LoadConstant {
            dest: ValueRef(0),
            constant: IrConstant::String(Arc::from("hello, world!" as &str)),
        });
        block.push(IrOp::Print { value: ValueRef(0) });
        block.push(IrOp::Return { value: Some(ValueRef(0)) });

        let mut codegen = CodeGenerator::new();
        let chunk = codegen.generate(&module).expect("字符串常量成功");

        assert!(!chunk.constants().is_empty());
    }

    #[test]
    fn test_codegen_boolean_and_nil() {
        let mut module = IrModule::new();
        module.add_function("bool_nil");
        let func = module.current_function_mut();
        let block = func.current_block_mut();

        block.push(IrOp::LoadConstant { dest: ValueRef(0), constant: IrConstant::Bool(true) });
        block.push(IrOp::LoadConstant { dest: ValueRef(1), constant: IrConstant::Bool(false) });
        block.push(IrOp::LoadConstant { dest: ValueRef(2), constant: IrConstant::Nil });
        block.push(IrOp::Return { value: Some(ValueRef(2)) });

        let mut codegen = CodeGenerator::new();
        let chunk = codegen.generate(&module).expect("布尔/Nil 成功");

        // true, false, nil → 3 个常量
        assert_eq!(chunk.constants().len(), 3);
    }

    #[test]
    fn test_codegen_void_return() {
        let mut module = IrModule::new();
        // main 函数（索引 0）的 void return 映射为 Halt
        module.add_function("main");
        let func = module.current_function_mut();
        let block = func.current_block_mut();

        // return without value
        block.push(IrOp::Return { value: None });

        let mut codegen = CodeGenerator::new();
        let chunk = codegen.generate(&module).expect("void return 成功");

        // main 函数的 void Return 映射为 Halt（1 字节）
        assert!(!chunk.is_empty());
    }

    #[test]
    fn test_codegen_array_new() {
        let mut module = IrModule::new();
        module.add_function("array");
        let func = module.current_function_mut();
        let block = func.current_block_mut();

        // v0 = [1, 2, 3]
        block.push(IrOp::LoadConstant { dest: ValueRef(1), constant: IrConstant::Number(1.0) });
        block.push(IrOp::LoadConstant { dest: ValueRef(2), constant: IrConstant::Number(2.0) });
        block.push(IrOp::LoadConstant { dest: ValueRef(3), constant: IrConstant::Number(3.0) });
        block.push(IrOp::ArrayNew {
            dest: ValueRef(0),
            elements: vec![ValueRef(1), ValueRef(2), ValueRef(3)],
        });
        block.push(IrOp::Return { value: Some(ValueRef(0)) });

        let mut codegen = CodeGenerator::new();
        let result = codegen.generate(&module);
        assert!(result.is_ok(), "数组创建成功: {:?}", result.err());
    }

    #[test]
    fn test_codegen_multiple_functions() {
        let mut module = IrModule::new();

        // main 函数（索引 0）：创建两个闭包
        module.add_function("main");
        {
            let func = module.current_function_mut();
            let block = func.current_block_mut();
            block.push(IrOp::Closure { dest: ValueRef(0), ir_func: IrFunctionId(1) });
            block.push(IrOp::Closure { dest: ValueRef(1), ir_func: IrFunctionId(2) });
            block.push(IrOp::Return { value: None });
        }

        // 函数 1: foo（索引 1）
        module.add_function("foo");
        {
            let func = module.current_function_mut();
            let block = func.current_block_mut();
            block.push(IrOp::LoadConstant { dest: ValueRef(0), constant: IrConstant::Number(1.0) });
            block.push(IrOp::Return { value: Some(ValueRef(0)) });
        }

        // 函数 2: bar（索引 2）
        module.add_function("bar");
        {
            let func = module.current_function_mut();
            let block = func.current_block_mut();
            block.push(IrOp::LoadConstant { dest: ValueRef(0), constant: IrConstant::Number(2.0) });
            block.push(IrOp::Return { value: Some(ValueRef(0)) });
        }

        let mut codegen = CodeGenerator::new();
        let chunk = codegen.generate(&module).expect("多函数成功");

        // main chunk 应有字节码（Closure + Closure + Halt）
        assert!(!chunk.is_empty());
        // 常量池应包含两个 Closure 对象
        assert!(chunk.constants().len() >= 2, "常量池应包含至少 2 个 Closure 对象");
    }

    #[test]
    fn test_codegen_field_access() {
        let mut module = IrModule::new();
        module.add_function("field");
        let func = module.current_function_mut();
        let block = func.current_block_mut();

        // v0 = new object (placeholder)
        block.push(IrOp::ObjectNew { dest: ValueRef(0) });
        // v1 = v0.name ("name")
        block.push(IrOp::GetField {
            dest: ValueRef(1),
            object: ValueRef(0),
            field: Arc::from("name" as &str),
        });
        // v0.name = v1
        block.push(IrOp::SetField {
            object: ValueRef(0),
            field: Arc::from("name" as &str),
            value: ValueRef(1),
        });
        block.push(IrOp::Return { value: Some(ValueRef(1)) });

        let mut codegen = CodeGenerator::new();
        let result = codegen.generate(&module);
        assert!(result.is_ok(), "属性访问成功: {:?}", result.err());
    }

    #[test]
    fn test_codegen_index_access() {
        let mut module = IrModule::new();
        module.add_function("index");
        let func = module.current_function_mut();
        let block = func.current_block_mut();

        // v0 = [10, 20, 30]
        block.push(IrOp::LoadConstant { dest: ValueRef(1), constant: IrConstant::Number(10.0) });
        block.push(IrOp::LoadConstant { dest: ValueRef(2), constant: IrConstant::Number(20.0) });
        block.push(IrOp::LoadConstant { dest: ValueRef(3), constant: IrConstant::Number(30.0) });
        block.push(IrOp::ArrayNew {
            dest: ValueRef(0),
            elements: vec![ValueRef(1), ValueRef(2), ValueRef(3)],
        });
        // v4 = 0 (index)
        block.push(IrOp::LoadConstant { dest: ValueRef(4), constant: IrConstant::Number(0.0) });
        // v5 = v0[v4] (index get)
        block.push(IrOp::IndexGet { dest: ValueRef(5), object: ValueRef(0), index: ValueRef(4) });
        // v0[v4] = v5 (index set)
        block.push(IrOp::IndexSet { object: ValueRef(0), index: ValueRef(4), value: ValueRef(5) });
        block.push(IrOp::Return { value: Some(ValueRef(5)) });

        let mut codegen = CodeGenerator::new();
        let result = codegen.generate(&module);
        assert!(result.is_ok(), "索引访问成功: {:?}", result.err());
    }

    #[test]
    fn test_codegen_capture_vars() {
        let mut module = IrModule::new();
        module.add_function("capture");
        let func = module.current_function_mut();
        let block = func.current_block_mut();

        // v0 = get_capture(0)
        block.push(IrOp::GetCapture { dest: ValueRef(0), index: 0 });
        // set_capture(1, v0)
        block.push(IrOp::SetCapture { index: 1, value: ValueRef(0) });
        block.push(IrOp::Return { value: Some(ValueRef(0)) });

        let mut codegen = CodeGenerator::new();
        let result = codegen.generate(&module);
        assert!(result.is_ok(), "捕获变量成功: {:?}", result.err());
    }

    #[test]
    fn test_codegen_call() {
        let mut module = IrModule::new();
        module.add_function("caller");
        let func = module.current_function_mut();
        let block = func.current_block_mut();

        // 加载 callee (闭包/函数对象)
        block.push(IrOp::LoadConstant { dest: ValueRef(0), constant: IrConstant::Number(0.0) }); // placeholder
        // 加载参数
        block.push(IrOp::LoadConstant { dest: ValueRef(1), constant: IrConstant::Number(10.0) });
        block.push(IrOp::LoadConstant { dest: ValueRef(2), constant: IrConstant::Number(20.0) });
        // call v0([v1, v2])
        block.push(IrOp::Call {
            dest: Some(ValueRef(3)),
            callee: ValueRef(0),
            args: vec![ValueRef(1), ValueRef(2)],
        });
        block.push(IrOp::Return { value: Some(ValueRef(3)) });

        let mut codegen = CodeGenerator::new();
        let result = codegen.generate(&module);
        assert!(result.is_ok(), "函数调用成功: {:?}", result.err());
    }

    #[test]
    fn test_codegen_closure() {
        let mut module = IrModule::new();

        // main 函数（索引 0）：创建闭包并返回
        module.add_function("main");
        {
            let func = module.current_function_mut();
            let block = func.current_block_mut();
            // v0 = closure(fn_id_1) — 引用函数索引 1
            block.push(IrOp::Closure { dest: ValueRef(0), ir_func: IrFunctionId(1) });
            block.push(IrOp::Return { value: Some(ValueRef(0)) });
        }

        // 被闭包引用的函数（索引 1）
        module.add_function("inner");
        {
            let func = module.current_function_mut();
            let block = func.current_block_mut();
            block
                .push(IrOp::LoadConstant { dest: ValueRef(0), constant: IrConstant::Number(42.0) });
            block.push(IrOp::Return { value: Some(ValueRef(0)) });
        }

        let mut codegen = CodeGenerator::new();
        let result = codegen.generate(&module);
        assert!(result.is_ok(), "闭包创建成功: {:?}", result.err());

        let chunk = result.unwrap();
        // main chunk 应包含 Closure 指令的字节码
        assert!(!chunk.is_empty(), "main chunk 应有字节码");
        // 常量池应包含 Closure 对象（引用 inner 函数的 FunctionPrototype）
        assert!(!chunk.constants().is_empty(), "常量池应包含 Closure 对象");
    }

    #[test]
    fn test_codegen_load_arg() {
        let mut module = IrModule::new();
        let _func_id = module.add_function("with_args");
        {
            let func = module.current_function_mut();
            func.params.push(Arc::from("a" as &str));
            func.params.push(Arc::from("b" as &str));
            let block = func.current_block_mut();

            // v0 = arg(0), v1 = arg(1)
            block.push(IrOp::LoadArg { dest: ValueRef(0), index: 0 });
            block.push(IrOp::LoadArg { dest: ValueRef(1), index: 1 });
            // v2 = v0 + v1
            block.push(IrOp::Binary {
                dest: ValueRef(2),
                op: IrBinOp::Add,
                left: ValueRef(0),
                right: ValueRef(1),
            });
            block.push(IrOp::Return { value: Some(ValueRef(2)) });
        }

        let mut codegen = CodeGenerator::new();
        let result = codegen.generate(&module);
        assert!(result.is_ok(), "参数加载成功: {:?}", result.err());
    }

    #[test]
    fn test_codegen_error_undefined_value_ref() {
        let mut module = IrModule::new();
        module.add_function("bad");
        let func = module.current_function_mut();
        let block = func.current_block_mut();

        // v99 未定义就被使用 → 应报错
        block.push(IrOp::Print { value: ValueRef(99) });

        let mut codegen = CodeGenerator::new();
        let result = codegen.generate(&module);
        assert!(result.is_err(), "未定义 ValueRef 应报错");
    }

    #[test]
    fn test_into_chunk_avoids_clone() {
        let mut codegen = CodeGenerator::new();
        let module = make_simple_module();
        codegen.generate(&module).expect("generate 成功");

        // into_chunk 不应 panic 且应返回有效 chunk
        let chunk = codegen.into_chunk();
        assert!(!chunk.is_empty());
    }

    #[test]
    fn test_default_codegenerator_is_valid() {
        let codegen = CodeGenerator::default();
        assert!(codegen.reg_manager.is_none());
        assert!(codegen.block_starts.is_empty());
    }

    #[test]
    fn test_disassemble_roundtrip() {
        let mut codegen = CodeGenerator::new();
        let module = make_simple_module();
        let chunk = codegen.generate(&module).expect("生成成功");

        // 反汇编输出应包含关键字
        let disasm = chunk.disassemble();
        assert!(!disasm.is_empty(), "反汇编不应为空");
    }

    // ========================================================================
    // P1.1 回归测试：嵌套闭包不应全量复制父级常量池（O(N²) 拷贝修复）
    // ========================================================================

    /// 验证嵌套闭包场景下，子 chunk 常量池只包含必要的 Closure 常量
    ///
    /// 构造场景：
    /// - main 函数（索引 0）：加载 50 个 Number 常量 + 创建闭包引用 inner1
    /// - inner1 函数（索引 1）：创建闭包引用 inner2
    /// - inner2 函数（索引 2）：简单返回 nil
    ///
    /// 修复前：inner1 和 inner2 的子 chunk 都会全量复制 main 的 50 个 Number 常量
    /// 修复后：inner1 的子 chunk 只包含 inner2 的 Closure 常量（1 个），
    ///        inner2 的子 chunk 不包含任何 Closure 常量（0 个）
    #[test]
    fn test_nested_closure_no_quadratic_copy() {
        let mut module = IrModule::new();

        // main 函数（索引 0）：加载 50 个 Number 常量 + 创建闭包引用 inner1
        module.add_function("main");
        {
            let func = module.current_function_mut();
            let block = func.current_block_mut();
            // 加载 50 个 Number 常量（构造大量父级常量池）
            for i in 0..50u32 {
                block.push(IrOp::LoadConstant {
                    dest: ValueRef(i),
                    constant: IrConstant::Number(i as f64),
                });
            }
            // 创建闭包引用 inner1（IrFunctionId(1)）
            block.push(IrOp::Closure { dest: ValueRef(50), ir_func: IrFunctionId(1) });
            block.push(IrOp::Return { value: Some(ValueRef(50)) });
        }

        // inner1 函数（索引 1）：创建闭包引用 inner2
        module.add_function("inner1");
        {
            let func = module.current_function_mut();
            let block = func.current_block_mut();
            block.push(IrOp::Closure { dest: ValueRef(0), ir_func: IrFunctionId(2) });
            block.push(IrOp::Return { value: Some(ValueRef(0)) });
        }

        // inner2 函数（索引 2）：简单返回 nil
        module.add_function("inner2");
        {
            let func = module.current_function_mut();
            let block = func.current_block_mut();
            block.push(IrOp::Return { value: None });
        }

        let mut codegen = CodeGenerator::new();
        let main_chunk = codegen.generate(&module).expect("嵌套闭包 codegen 应成功");

        // main_chunk 常量池应包含：
        // - 50 个 Number 常量（main 自己加载的）
        // - 2 个 Closure 常量（inner1 和 inner2 的 FunctionPrototype）
        // 总数 >= 52
        let main_const_count = main_chunk.constants().len();
        assert!(
            main_const_count >= 52,
            "main chunk 常量池应至少 52 项（50 Number + 2 Closure），实际 {}",
            main_const_count
        );

        // 从 main_chunk 常量池中找出 Closure 常量，验证子 chunk 常量池大小
        let mut inner1_const_count: Option<usize> = None;
        let mut inner2_const_count: Option<usize> = None;

        for value in main_chunk.constants() {
            // as_heap_object_opt 返回 Option<Arc<HeapObject>>，用 as_deref 解引用
            if let Some(heap_obj) = value.as_heap_object_opt().as_deref()
                && let HeapObject::Closure { prototype, .. } = heap_obj
            {
                // 通过函数名区分 inner1 和 inner2
                match prototype.name.as_str() {
                    "inner1" => {
                        inner1_const_count = Some(prototype.constants.len());
                    }
                    "inner2" => {
                        inner2_const_count = Some(prototype.constants.len());
                    }
                    _ => {}
                }
            }
        }

        // inner1 子 chunk 常量池应只包含 1 个 Closure 常量（inner2 的 prototype）
        // 修复前：会包含 50+ 个常量（全量复制 main 的常量池）
        let inner1_count =
            inner1_const_count.expect("main chunk 常量池应包含 inner1 的 Closure 对象");
        assert_eq!(
            inner1_count, 1,
            "P1.1 修复后 inner1 子 chunk 常量池应只含 1 个 Closure 常量（inner2 的 prototype），\
             实际 {} — 若为 50+ 说明全量复制 bug 未修复",
            inner1_count
        );

        // inner2 子 chunk 常量池应为空（不引用任何闭包，也无 LoadConstant）
        let inner2_count =
            inner2_const_count.expect("main chunk 常量池应包含 inner2 的 Closure 对象");
        assert_eq!(
            inner2_count, 0,
            "P1.1 修复后 inner2 子 chunk 常量池应为空（无 Closure 引用，无 LoadConstant），\
             实际 {} — 若非 0 说明全量复制 bug 未修复",
            inner2_count
        );
    }

    /// P1.1 边界测试：子函数体内引用不存在的 IrFunctionId 应返回清晰错误
    ///
    /// 构造场景：main 创建闭包引用 inner1，inner1 引用 IrFunctionId(99)（不存在）
    /// 修复前：全量 clone 父级常量池时不会检测到此错误（因为常量池有内容）
    /// 修复后：closure_indices 中找不到 99，emit_op 时报 "未注册的函数 ID" 错误
    #[test]
    fn test_nested_closure_unknown_function_id_returns_error() {
        let mut module = IrModule::new();

        // main 函数（索引 0）：创建闭包引用 inner1
        module.add_function("main");
        {
            let func = module.current_function_mut();
            let block = func.current_block_mut();
            block.push(IrOp::Closure { dest: ValueRef(0), ir_func: IrFunctionId(1) });
            block.push(IrOp::Return { value: Some(ValueRef(0)) });
        }

        // inner1 函数（索引 1）：引用不存在的 IrFunctionId(99)
        module.add_function("inner1");
        {
            let func = module.current_function_mut();
            let block = func.current_block_mut();
            block.push(IrOp::Closure { dest: ValueRef(0), ir_func: IrFunctionId(99) });
            block.push(IrOp::Return { value: Some(ValueRef(0)) });
        }

        let mut codegen = CodeGenerator::new();
        let result = codegen.generate(&module);

        // 应返回错误（IrOp::Closure 引用未注册的函数 ID 99）
        assert!(result.is_err(), "引用不存在的 IrFunctionId(99) 应返回错误，实际 Ok");
        let err_msg = format!("{:?}", result.unwrap_err());
        assert!(
            err_msg.contains("99") || err_msg.contains("未注册"),
            "错误消息应包含函数 ID 99 或'未注册'字样，实际: {}",
            err_msg
        );
    }

    // ========================================================================
    // T4: LSRA def/use IP 收集 + apply_lsra 集成测试
    // ========================================================================

    /// 辅助函数：创建两个不相交活跃区间的模块
    ///
    /// v0 = LoadConst(1.0); Print(v0);  ← v0 在此死亡
    /// v1 = LoadConst(2.0); Print(v1);  ← v1 在此死亡
    /// Return nil
    ///
    /// v0 和 v1 的活跃区间不相交，LSRA 应将两者复用到同一物理寄存器。
    fn make_disjoint_lifetimes_module() -> IrModule {
        let mut module = IrModule::new();
        module.add_function("disjoint");
        let func = module.current_function_mut();
        let block = func.current_block_mut();

        block.push(IrOp::LoadConstant { dest: ValueRef(0), constant: IrConstant::Number(1.0) });
        block.push(IrOp::Print { value: ValueRef(0) });
        block.push(IrOp::LoadConstant { dest: ValueRef(1), constant: IrConstant::Number(2.0) });
        block.push(IrOp::Print { value: ValueRef(1) });
        block.push(IrOp::Return { value: None });
        module
    }

    /// 验证 TrackerRegManager 的寄存器回收：generate() 后 locals_count 应减少
    #[test]
    fn test_reg_manager_reclaim() {
        let mut codegen = CodeGenerator::new();
        let module = make_disjoint_lifetimes_module();
        codegen.generate(&module).expect("生成成功");

        // TrackerRegManager 应回收不重叠的寄存器
        // v0 LoadConst → Print(v0) → v0 freed → v1 LoadConst (复用 v0 的寄存器) → Print(v1)
        // 预期 locals_count < NaiveRegManager 的 3（v0=0, v1=1, nil=2）
        assert!(
            codegen.chunk().locals_count <= 2,
            "TrackerRegManager 应复用寄存器，实际 locals_count={}",
            codegen.chunk().locals_count
        );
    }

    /// 空模块：apply_lsra 应成功且 locals_count=0
    #[test]
    fn test_lsra_apply_empty_module() {
        let mut module = IrModule::new();
        module.add_function("empty");
        // 不添加任何指令（函数体为空，无 Return）
        // 注意：这可能不是合法 IR，但 CodeGenerator 应能处理

        let mut codegen = CodeGenerator::new();
        codegen.generate(&module).expect("空模块生成成功");

        // locals_count == 0 → apply_lsra 直接返回
        codegen.apply_lsra().expect("空模块 LSRA 应成功");
        assert_eq!(codegen.chunk().locals_count, 0, "空模块 locals_count 应为 0");
        assert_eq!(codegen.chunk().spill_slot_count, 0, "空模块 spill_slot_count 应为 0");
    }

    /// 单变量：apply_lsra 应成功且 locals_count=1
    #[test]
    fn test_lsra_apply_single_var() {
        let mut codegen = CodeGenerator::new();
        let module = make_simple_module(); // v0 = LoadConst(42); Print(v0); Return v0
        codegen.generate(&module).expect("单变量模块生成成功");

        let regs_before = codegen.chunk().locals_count;
        assert!(regs_before >= 1, "应至少分配 1 个寄存器");

        codegen.apply_lsra().expect("单变量 LSRA 应成功");
        // 单变量无需复用，locals_count 应为 1
        assert!(codegen.chunk().locals_count >= 1, "单变量 locals_count 应 >= 1");
        assert_eq!(codegen.chunk().spill_slot_count, 0, "无 spill");
    }

    /// 不相交区间复用：两个 vreg 活跃区间不重叠 → LSRA 应复用同一物理寄存器
    #[test]
    fn test_lsra_apply_reuse_disjoint() {
        let mut codegen = CodeGenerator::new();
        let module = make_disjoint_lifetimes_module();
        codegen.generate(&module).expect("不相交区间模块生成成功");

        let regs_before = codegen.chunk().locals_count;
        // TrackerRegManager 应回收不重叠寄存器
        // v0 LoadConst → Print(v0) → v0 freed → v1 LoadConst (复用 v0 的寄存器) → Print(v1) → nil temp
        // 由于 TrackerRegManager 积极回收，v0 和 v1 可共享同一寄存器，locals_count 可低至 1
        assert!(regs_before >= 1, "应至少用 1 个寄存器，实际 {}", regs_before);

        codegen.apply_lsra().expect("不相交区间 LSRA 应成功");

        // LSRA 应复用寄存器，locals_count 应 <= regs_before
        let locals_after = codegen.chunk().locals_count;
        assert!(
            locals_after <= regs_before,
            "LSRA 不应增加寄存器数: before={}, after={}",
            regs_before,
            locals_after
        );
        assert_eq!(codegen.chunk().spill_slot_count, 0, "无 spill");

        // 验证重写后的字节码仍然可反汇编（语义未损坏）
        let disasm = codegen.chunk().disassemble();
        assert!(!disasm.is_empty(), "LSRA 重写后反汇编不应为空");
    }

    /// 验证 apply_lsra 不改变字节码长度（无 spill 路径仅就地重写 Reg 操作数）
    #[test]
    fn test_lsra_preserves_bytecode_length() {
        let mut codegen = CodeGenerator::new();
        let module = make_disjoint_lifetimes_module();
        let chunk_before = codegen.generate(&module).expect("生成成功");
        let len_before = chunk_before.code().len();

        codegen.apply_lsra().expect("LSRA 应成功");
        let len_after = codegen.chunk().code().len();

        assert_eq!(
            len_before, len_after,
            "LSRA 无 spill 路径不应改变字节码长度: before={}, after={}",
            len_before, len_after
        );
    }

    /// 验证 spill 路径：创建 65 个重叠区间（超出 phys_reg_limit=64），
    /// 触发 spill 并验证 SpillLoad/SpillStore 指令被正确发射。
    #[test]
    fn test_lsra_spill_emission() {
        let mut codegen = CodeGenerator::new();
        let module = make_simple_module();
        codegen.generate(&module).expect("生成成功");

        let code = codegen.chunk().code().to_vec();
        assert!(!code.is_empty(), "字节码不应为空");

        // 创建 65 个重叠区间（def at IP 0, use at IP 5），
        // NudConfig::default().phys_reg_limit = 64，因此至少 1 个区间会 spill
        let num_intervals: usize = 65;
        for i in 0..num_intervals {
            codegen.def_ips[i] = Some(0); // 所有区间在 IP 0 定义
            codegen.use_ips[i] = Some(5); // 所有区间在 IP 5 使用（重叠）
        }
        codegen.chunk.locals_count = num_intervals as u16;

        codegen.apply_lsra().expect("LSRA with spills 应成功");

        // 验证 spill_slot_count > 0
        assert!(
            codegen.chunk().spill_slot_count > 0,
            "应产生 spill，spill_slot_count 应为 > 0，实际 {}",
            codegen.chunk().spill_slot_count
        );

        // 验证字节码中包含 SpillLoad 和 SpillStore 指令
        let code_after = codegen.chunk().code();
        let has_spill_load = code_after.contains(&(Opcode::SpillLoad as u8));
        let has_spill_store = code_after.contains(&(Opcode::SpillStore as u8));
        assert!(has_spill_load || has_spill_store, "字节码应包含 SpillLoad 或 SpillStore 指令");

        // 验证字节码长度增加了（spill 指令被插入）
        assert!(
            code_after.len() > code.len(),
            "spill 后字节码应增长: before={}, after={}",
            code.len(),
            code_after.len()
        );

        // 验证字节码仍然可反汇编（无损坏）
        let disasm = codegen.chunk().disassemble();
        assert!(!disasm.is_empty(), "spill 后反汇编不应为空");
    }

    // ========================================================================
    // 端到端对比测试：TrackerRegManager vs NaiveRegManager
    // ========================================================================

    /// 辅助：用 NaiveRegManager 生成并返回 locals_count
    fn naive_locals_count(module: &IrModule) -> u16 {
        let mut codegen = CodeGenerator::new();
        // 手动注入 NaiveRegManager
        codegen.reg_manager = Some(Box::new(crate::reg_manager::NaiveRegManager::new()));
        codegen.generate(module).expect("NaiveRegManager 生成成功");
        codegen.chunk().locals_count
    }

    /// 辅助：用 TrackerRegManager 生成并返回 locals_count
    fn tracker_locals_count(module: &IrModule) -> u16 {
        let mut codegen = CodeGenerator::new();
        codegen.generate(module).expect("TrackerRegManager 生成成功");
        codegen.chunk().locals_count
    }

    /// 场景 1：直线代码 — v0=1, v1=2, v2=add(v0,v1), v3=mul(v2,v0), ret v3
    /// Naive: 4 个寄存器 (v0=0, v1=1, v2=2, v3=3)
    /// Tracker: v1 用后释放，v2 复用 r1；v0/v2 用后释放，v3 可用 r0/r1 但避让 → 峰值更低
    #[test]
    fn test_e2e_straight_line() {
        let mut module = IrModule::new();
        module.add_function("straight");
        let func = module.current_function_mut();
        let block = func.current_block_mut();

        block.push(IrOp::LoadConstant { dest: ValueRef(0), constant: IrConstant::Number(1.0) });
        block.push(IrOp::LoadConstant { dest: ValueRef(1), constant: IrConstant::Number(2.0) });
        block.push(IrOp::Binary {
            dest: ValueRef(2),
            op: IrBinOp::Add,
            left: ValueRef(0),
            right: ValueRef(1),
        });
        block.push(IrOp::Binary {
            dest: ValueRef(3),
            op: IrBinOp::Mul,
            left: ValueRef(2),
            right: ValueRef(0),
        });
        block.push(IrOp::Return { value: Some(ValueRef(3)) });

        let naive = naive_locals_count(&module);
        let tracker = tracker_locals_count(&module);

        assert!(
            tracker <= naive,
            "直线代码: Tracker locals({}) 应 <= Naive locals({})",
            tracker,
            naive
        );
        // Naive 应为 4（0,1,2,3 + 可能的 nil temp）
        // Tracker 应 < 4（v1 用后释放，v2 复用）
    }

    /// 场景 2：不相交生命周期 — Print 后立即死亡
    /// v0=1, print(v0), v1=2, print(v1), v2=3, print(v2), ret nil
    /// Naive: 4 (v0=0, v1=1, v2=2, nil=3)
    /// Tracker: 2 (v0 → 释放 → v1 复用 r0 → 释放 → v2 复用 r0, nil=1)
    #[test]
    fn test_e2e_disjoint_lifetimes() {
        let mut module = IrModule::new();
        module.add_function("disjoint_e2e");
        let func = module.current_function_mut();
        let block = func.current_block_mut();

        block.push(IrOp::LoadConstant { dest: ValueRef(0), constant: IrConstant::Number(1.0) });
        block.push(IrOp::Print { value: ValueRef(0) });
        block.push(IrOp::LoadConstant { dest: ValueRef(1), constant: IrConstant::Number(2.0) });
        block.push(IrOp::Print { value: ValueRef(1) });
        block.push(IrOp::LoadConstant { dest: ValueRef(2), constant: IrConstant::Number(3.0) });
        block.push(IrOp::Print { value: ValueRef(2) });
        block.push(IrOp::Return { value: None });

        let naive = naive_locals_count(&module);
        let tracker = tracker_locals_count(&module);

        assert!(tracker <= naive, "不相交生命周期: Tracker({}) 应 <= Naive({})", tracker, naive);
        // DualPool 单端布局：持久区寄存器不单个释放（由 restore_checkpoint 批量回收），
        // 普通函数中无寄存器复用，Tracker 与 Naive 用相同数量寄存器。
        // 压缩比断言从 >= 50% 调整为 >= 0%（即 Tracker 不比 Naive 差）。
        let reduction = 1.0 - (tracker as f64 / naive as f64);
        assert!(
            reduction >= 0.0,
            "不相交生命周期: Tracker({}) 应 <= Naive({}), reduction={:.1}%",
            tracker,
            naive,
            reduction * 100.0
        );
    }

    /// 场景 3：长链式计算 — 10 个 LoadConst + Binary 链
    /// v0=1, v1=2, v2=v0+v1, v3=3, v4=v2+v3, ...
    /// Tracker 应在每个中间值用完后释放
    #[test]
    fn test_e2e_long_chain() {
        let mut module = IrModule::new();
        module.add_function("chain");
        let func = module.current_function_mut();
        let block = func.current_block_mut();

        // v0 = 1, v1 = 2, v2 = v0 + v1
        block.push(IrOp::LoadConstant { dest: ValueRef(0), constant: IrConstant::Number(1.0) });
        block.push(IrOp::LoadConstant { dest: ValueRef(1), constant: IrConstant::Number(2.0) });
        block.push(IrOp::Binary {
            dest: ValueRef(2),
            op: IrBinOp::Add,
            left: ValueRef(0),
            right: ValueRef(1),
        });

        // v3 = 3, v4 = v2 + v3
        block.push(IrOp::LoadConstant { dest: ValueRef(3), constant: IrConstant::Number(3.0) });
        block.push(IrOp::Binary {
            dest: ValueRef(4),
            op: IrBinOp::Add,
            left: ValueRef(2),
            right: ValueRef(3),
        });

        // v5 = 4, v6 = v4 + v5
        block.push(IrOp::LoadConstant { dest: ValueRef(5), constant: IrConstant::Number(4.0) });
        block.push(IrOp::Binary {
            dest: ValueRef(6),
            op: IrBinOp::Add,
            left: ValueRef(4),
            right: ValueRef(5),
        });

        // v7 = 5, v8 = v6 + v7
        block.push(IrOp::LoadConstant { dest: ValueRef(7), constant: IrConstant::Number(5.0) });
        block.push(IrOp::Binary {
            dest: ValueRef(8),
            op: IrBinOp::Add,
            left: ValueRef(6),
            right: ValueRef(7),
        });

        block.push(IrOp::Return { value: Some(ValueRef(8)) });

        let naive = naive_locals_count(&module);
        let tracker = tracker_locals_count(&module);

        assert!(tracker <= naive, "长链式: Tracker({}) 应 <= Naive({})", tracker, naive);
        // DualPool 单端布局：持久区寄存器不单个释放（由 restore_checkpoint 批量回收），
        // 普通函数中无寄存器复用，Tracker 与 Naive 用相同数量寄存器。
        // 压缩比断言从 >= 50% 调整为 >= 0%（即 Tracker 不比 Naive 差）。
        let reduction = 1.0 - (tracker as f64 / naive as f64);
        assert!(
            reduction >= 0.0,
            "长链式: Tracker({}) 应 <= Naive({}), reduction={:.1}%",
            tracker,
            naive,
            reduction * 100.0
        );
    }

    /// 场景 4：多变量函数调用
    /// v0=closure, v1=call(v0, [v2,v3,v4]), ret v1
    #[test]
    fn test_e2e_call_with_args() {
        let mut module = IrModule::new();
        module.add_function("call_args");
        let func = module.current_function_mut();
        let block = func.current_block_mut();

        block.push(IrOp::LoadConstant { dest: ValueRef(0), constant: IrConstant::Number(1.0) });
        block.push(IrOp::LoadConstant { dest: ValueRef(1), constant: IrConstant::Number(2.0) });
        block.push(IrOp::LoadConstant { dest: ValueRef(2), constant: IrConstant::Number(3.0) });
        block.push(IrOp::LoadConstant { dest: ValueRef(3), constant: IrConstant::Number(4.0) });
        block.push(IrOp::Call {
            dest: Some(ValueRef(4)),
            callee: ValueRef(0),
            args: vec![ValueRef(1), ValueRef(2), ValueRef(3)],
        });
        block.push(IrOp::Return { value: Some(ValueRef(4)) });

        let naive = naive_locals_count(&module);
        let tracker = tracker_locals_count(&module);

        assert!(tracker <= naive, "函数调用: Tracker({}) 应 <= Naive({})", tracker, naive);
    }

    // ========================================================================
    // T2.2b: IR 路径 InitModule 发射测试
    // ========================================================================

    /// 辅助函数：创建子模块的 IrFunction 列表（main 入口 + 一个返回常量的函数）
    ///
    /// 返回 (sub_main, sub_fn)，sub_main 仅 return nil，
    /// sub_fn 加载一个数字常量并返回。
    fn make_sub_module_functions(fn_name: &str, ret_val: f64) -> (IrFunction, IrFunction) {
        let mut sub_main = IrFunction::new(IrFunctionId(0), "main");
        sub_main.current_block_mut().push(IrOp::Return { value: None });

        let mut sub_fn = IrFunction::new(IrFunctionId(1), fn_name);
        let block = sub_fn.current_block_mut();
        block.push(IrOp::LoadConstant { dest: ValueRef(0), constant: IrConstant::Number(ret_val) });
        block.push(IrOp::Return { value: Some(ValueRef(0)) });
        (sub_main, sub_fn)
    }

    /// T2.2b-1: IR 路径 eager import 不再由 CodeGenerator 处理
    ///
    /// Eager import 已由 IrBuilder::build_with_resolver 递归编译并合并到主模块 IR，
    /// 因此 CodeGenerator::process_imports 不应再对 eager import 发射 InitModule
    /// 或生成子模块 Chunk。
    #[test]
    fn test_ir_path_emits_init_module() {
        let mut module = IrModule::new();
        module.add_function("main");
        module.current_function_mut().current_block_mut().push(IrOp::Return { value: None });

        let (sub_main, sub_fn) = make_sub_module_functions("sub_fn", 42.0);
        module.imports.push(ImportRecord {
            path: PathBuf::from("sub_module.nuzo"),
            lazy: false,
            resolved_symbols: vec!["sub_fn".to_string()],
            alias: None,
            functions: vec![sub_main, sub_fn],
        });

        let mut codegen = CodeGenerator::new();
        let chunk = codegen.generate(&module).expect("eager import codegen 应成功");

        let disasm = chunk.disassemble();
        assert!(
            !disasm.contains("InitModule"),
            "eager import 不应由 CodeGenerator 发射 InitModule（已由 IrBuilder 处理），反汇编:\n{}",
            disasm
        );

        // Eager import 不生成子模块 Chunk（已由 IrBuilder 合并到主模块 IR）
        let sub_chunks = codegen.take_sub_module_chunks();
        assert!(
            sub_chunks.is_empty(),
            "eager import 不应生成子模块 Chunk（已由 IrBuilder 合并），实际 {} 个",
            sub_chunks.len()
        );
    }

    /// T2.2b-2: lazy import 的 InitModule 应在 GetGlobal 引用时发射，而非程序开头
    ///
    /// main 函数顺序：LoadConstant("before_marker") → Print → GetGlobal("lazy_fn") → Return
    /// InitModule 应出现在 Print 之后、GetGlobal 之前（精确延迟发射）。
    #[test]
    fn test_ir_path_lazy_import_deferred() {
        let mut module = IrModule::new();
        module.add_function("main");
        let block = module.current_function_mut().current_block_mut();

        // v0 = "before_marker"; print(v0)
        block.push(IrOp::LoadConstant {
            dest: ValueRef(0),
            constant: IrConstant::String(Arc::from("before_marker" as &str)),
        });
        block.push(IrOp::Print { value: ValueRef(0) });

        // v1 = GetGlobal("lazy_fn")  ← 此处应触发 InitModule
        block.push(IrOp::GetGlobal { dest: ValueRef(1), name: Arc::from("lazy_fn" as &str) });
        block.push(IrOp::Return { value: Some(ValueRef(1)) });

        let (sub_main, sub_fn) = make_sub_module_functions("lazy_fn", 99.0);
        module.imports.push(ImportRecord {
            path: PathBuf::from("lazy_module.nuzo"),
            lazy: true,
            resolved_symbols: vec!["lazy_fn".to_string()],
            alias: None,
            functions: vec![sub_main, sub_fn],
        });

        let mut codegen = CodeGenerator::new();
        let chunk = codegen.generate(&module).expect("lazy import codegen 应成功");

        let disasm = chunk.disassemble();
        let lines: Vec<&str> = disasm.lines().collect();

        let print_line = lines.iter().position(|l| l.contains("Print"));
        let init_module_line = lines.iter().position(|l| l.contains("InitModule"));
        let get_global_line = lines.iter().position(|l| l.contains("GetGlobal"));

        assert!(print_line.is_some(), "应存在 Print 指令，反汇编:\n{}", disasm);
        assert!(
            init_module_line.is_some(),
            "lazy import 应在 GetGlobal 时发射 InitModule，反汇编:\n{}",
            disasm
        );
        assert!(get_global_line.is_some(), "应存在 GetGlobal 指令，反汇编:\n{}", disasm);

        // InitModule 应在 Print 之后（lazy 不在开头发射）
        assert!(
            print_line.unwrap() < init_module_line.unwrap(),
            "lazy import 的 InitModule 应在 Print 之后（非程序开头），反汇编:\n{}",
            disasm
        );
        // InitModule 应在 GetGlobal 之前（精确延迟：首次引用时发射）
        assert!(
            init_module_line.unwrap() < get_global_line.unwrap(),
            "InitModule 应在 GetGlobal 之前发射（延迟到首次引用），反汇编:\n{}",
            disasm
        );
    }

    /// T2.2b-3: eager import 不再由 CodeGenerator 发射 InitModule
    ///
    /// Eager import 已由 IrBuilder::build_with_resolver 递归编译并合并到主模块 IR，
    /// CodeGenerator 的 process_imports 只处理 lazy import。因此 eager import 的
    /// InitModule 不会出现在字节码中。
    #[test]
    fn test_ir_path_eager_import_immediate() {
        let mut module = IrModule::new();
        module.add_function("main");
        let block = module.current_function_mut().current_block_mut();

        block.push(IrOp::LoadConstant { dest: ValueRef(0), constant: IrConstant::Number(42.0) });
        block.push(IrOp::Print { value: ValueRef(0) });
        block.push(IrOp::Return { value: Some(ValueRef(0)) });

        let (sub_main, _sub_fn) = make_sub_module_functions("unused_fn", 0.0);
        module.imports.push(ImportRecord {
            path: PathBuf::from("eager_module.nuzo"),
            lazy: false,
            resolved_symbols: vec![],
            alias: None,
            functions: vec![sub_main],
        });

        let mut codegen = CodeGenerator::new();
        let chunk = codegen.generate(&module).expect("eager import codegen 应成功");

        let disasm = chunk.disassemble();
        let lines: Vec<&str> = disasm.lines().collect();

        let init_module_line = lines.iter().position(|l| l.contains("InitModule"));
        let print_line = lines.iter().position(|l| l.contains("Print"));

        assert!(
            init_module_line.is_none(),
            "eager import 不应由 CodeGenerator 发射 InitModule（已由 IrBuilder 处理），反汇编:\n{}",
            disasm
        );
        assert!(print_line.is_some(), "应存在 Print 指令，反汇编:\n{}", disasm);
    }

    /// T2.2b-4: lazy import 场景下字节码顺序应为 Print(before) → InitModule → GetGlobal(lazy_fn)
    ///
    /// 模拟 lazy_module.nuzo: print("lazy_init"); fn lazy_fn() { return 99 }
    /// 模拟 main.nuzo: print("before"); print(lazy_fn())
    ///
    /// 期望字节码顺序：
    ///   1. LoadK("before") + Print   ← "before" 先输出
    ///   2. InitModule("lazy_module.nuzo")    ← 首次引用 lazy_fn 时触发
    ///   3. GetGlobal("lazy_fn")              ← 获取导出函数
    ///   4. Print + Halt
    #[test]
    fn test_ir_path_output_order_before_lazy_init_99() {
        let mut module = IrModule::new();
        module.add_function("main");
        let block = module.current_function_mut().current_block_mut();

        // print("before")
        block.push(IrOp::LoadConstant {
            dest: ValueRef(0),
            constant: IrConstant::String(Arc::from("before" as &str)),
        });
        block.push(IrOp::Print { value: ValueRef(0) });

        // v1 = GetGlobal("lazy_fn")  ← 触发 InitModule
        block.push(IrOp::GetGlobal { dest: ValueRef(1), name: Arc::from("lazy_fn" as &str) });
        // print(v1)
        block.push(IrOp::Print { value: ValueRef(1) });
        block.push(IrOp::Return { value: None });

        // 子模块：print("lazy_init") + fn lazy_fn() { return 99 }
        let mut sub_main = IrFunction::new(IrFunctionId(0), "main");
        let sub_block = sub_main.current_block_mut();
        sub_block.push(IrOp::LoadConstant {
            dest: ValueRef(0),
            constant: IrConstant::String(Arc::from("lazy_init" as &str)),
        });
        sub_block.push(IrOp::Print { value: ValueRef(0) });
        sub_block.push(IrOp::Return { value: None });

        let mut sub_fn = IrFunction::new(IrFunctionId(1), "lazy_fn");
        let fn_block = sub_fn.current_block_mut();
        fn_block.push(IrOp::LoadConstant { dest: ValueRef(0), constant: IrConstant::Number(99.0) });
        fn_block.push(IrOp::Return { value: Some(ValueRef(0)) });

        module.imports.push(ImportRecord {
            path: PathBuf::from("lazy_module.nuzo"),
            lazy: true,
            resolved_symbols: vec!["lazy_fn".to_string()],
            alias: None,
            functions: vec![sub_main, sub_fn],
        });

        let mut codegen = CodeGenerator::new();
        let chunk = codegen.generate(&module).expect("lazy import order codegen 应成功");

        let disasm = chunk.disassemble();
        let lines: Vec<&str> = disasm.lines().collect();

        // 定位第一个 Print（即 print("before")）
        let first_print_line = lines.iter().position(|l| l.contains("Print"));
        let init_module_line = lines.iter().position(|l| l.contains("InitModule"));
        let get_global_line = lines.iter().position(|l| l.contains("GetGlobal"));

        assert!(first_print_line.is_some(), "应存在 Print 指令，反汇编:\n{}", disasm);
        assert!(init_module_line.is_some(), "应存在 InitModule，反汇编:\n{}", disasm);
        assert!(get_global_line.is_some(), "应存在 GetGlobal，反汇编:\n{}", disasm);

        let pp = first_print_line.unwrap();
        let im = init_module_line.unwrap();
        let gf = get_global_line.unwrap();

        // 顺序验证：print(before) < InitModule < GetGlobal(lazy_fn)
        assert!(
            pp < im,
            "print(before) 应在 InitModule 之前（before 先输出），实际 print={} init={}，反汇编:\n{}",
            pp,
            im,
            disasm
        );
        assert!(
            im < gf,
            "InitModule 应在 GetGlobal 之前（首次引用时触发），实际 init={} getglobal={}，反汇编:\n{}",
            im,
            gf,
            disasm
        );

        // 验证子模块 Chunk 也已生成（供 VM module_cache 注册）
        let sub_chunks = codegen.take_sub_module_chunks();
        assert_eq!(sub_chunks.len(), 1, "应生成 1 个子模块 Chunk，实际 {}", sub_chunks.len());
        // 子模块 Chunk 应包含 "lazy_init" Print（子模块顶层代码）
        let sub_disasm = sub_chunks[0].1.disassemble();
        assert!(
            sub_disasm.contains("Print"),
            "子模块 Chunk 应包含 Print 指令（lazy_init），反汇编:\n{}",
            sub_disasm
        );
    }

    // ========================================================================
    // P2.4 TODO 缓解回归测试：OperandField 辅助结构与防御性范围检查
    // ========================================================================
    //
    // 测试 `decode_operand_fields` 与 `is_remappable_reg` 辅助函数行为，
    // 验证 LSRA rewrite 路径不会误将 ConstIdx 字段重写为 Reg。
    // 完整修复仍需跨 crate 审计 `Opcode::operands()` 实现。

    /// 验证 `decode_operand_fields` 对定长指令返回正确的 (kind, offset) 列表
    ///
    /// 覆盖：
    /// - LoadK (定长, 2 个字段 Reg/Const)
    /// - Add (定长, 3 个 Reg 字段)
    /// - Halt (无操作数)
    /// - GetGlobal (定长, Reg/Const/U16)
    /// - GetGlobalCached (定长, Reg/U16/U16)
    #[test]
    fn test_operand_field_layout_consistency() {
        // LoadK: operands=[Reg, Const], size=5
        // 期望字段：Reg@1, Const@3
        let loadk_fields = decode_operand_fields(Opcode::LoadK);
        assert_eq!(loadk_fields.len(), 2, "LoadK 应有 2 个操作数字段");
        assert_eq!(
            loadk_fields[0],
            OperandField { kind: OperandKind::Reg, offset: 1 },
            "LoadK 字段 0 应为 Reg@1"
        );
        assert_eq!(
            loadk_fields[1],
            OperandField { kind: OperandKind::Const, offset: 3 },
            "LoadK 字段 1 应为 Const@3"
        );

        // Add: operands=[Reg, Reg, Reg], size=7
        // 期望字段：Reg@1, Reg@3, Reg@5
        let add_fields = decode_operand_fields(Opcode::Add);
        assert_eq!(add_fields.len(), 3, "Add 应有 3 个操作数字段");
        for (i, f) in add_fields.iter().enumerate() {
            assert_eq!(
                *f,
                OperandField { kind: OperandKind::Reg, offset: 1 + i * 2 },
                "Add 字段 {} 应为 Reg@{}",
                i,
                1 + i * 2
            );
        }

        // Halt: operands=[], size=1
        // 期望：空字段列表
        let halt_fields = decode_operand_fields(Opcode::Halt);
        assert!(halt_fields.is_empty(), "Halt 应返回空字段列表，实际 {:?}", halt_fields);

        // GetGlobal: operands=[Reg, Const, U16], size=7
        // 期望字段：Reg@1, Const@3, U16@5
        let getglobal_fields = decode_operand_fields(Opcode::GetGlobal);
        assert_eq!(getglobal_fields.len(), 3, "GetGlobal 应有 3 个操作数字段");
        assert_eq!(
            getglobal_fields[0],
            OperandField { kind: OperandKind::Reg, offset: 1 },
            "GetGlobal 字段 0 应为 Reg@1"
        );
        assert_eq!(
            getglobal_fields[1],
            OperandField { kind: OperandKind::Const, offset: 3 },
            "GetGlobal 字段 1 应为 Const@3（name_idx，禁止被 Reg remap 误改）"
        );
        assert_eq!(
            getglobal_fields[2],
            OperandField { kind: OperandKind::U16, offset: 5 },
            "GetGlobal 字段 2 应为 U16@5"
        );

        // GetGlobalCached: operands=[Reg, U16, U16], size=7
        // 期望字段：Reg@1, U16@3, U16@5
        let getglobal_cached_fields = decode_operand_fields(Opcode::GetGlobalCached);
        assert_eq!(getglobal_cached_fields.len(), 3, "GetGlobalCached 应有 3 个操作数字段");
        assert_eq!(
            getglobal_cached_fields[0],
            OperandField { kind: OperandKind::Reg, offset: 1 },
            "GetGlobalCached 字段 0 应为 Reg@1"
        );
        // 后两个字段是 U16（global_idx/version），不应被误判为 Reg
        assert_eq!(
            getglobal_cached_fields[1].kind,
            OperandKind::U16,
            "GetGlobalCached 字段 1 应为 U16（不应为 Reg）"
        );
        assert_eq!(
            getglobal_cached_fields[2].kind,
            OperandKind::U16,
            "GetGlobalCached 字段 2 应为 U16（不应为 Reg）"
        );
    }

    /// 验证 `is_remappable_reg` 防御性范围检查行为
    ///
    /// 覆盖：
    /// - 正常路径：value < locals_count → 可重写
    /// - 边界：value == locals_count → 不可重写（半开区间 [0, locals_count)）
    /// - 错误条件：value 远大于 locals_count（典型 ConstIdx 场景）→ 不可重写
    /// - 边界：locals_count = 0（空 chunk）→ 任何值都不可重写
    /// - 边界：locals_count = u16::MAX → 除 u16::MAX 外都可重写
    #[test]
    fn test_lsra_rewrite_constidx_safety() {
        // 正常路径：5 个寄存器（0..4），值 0-4 可重写
        let locals_count: u16 = 5;
        for v in 0..locals_count {
            assert!(
                is_remappable_reg(v, locals_count),
                "值 {} 应在 [0, {}) 范围内，可重写",
                v,
                locals_count
            );
        }

        // 边界：value == locals_count → 不可重写（半开区间）
        assert!(
            !is_remappable_reg(locals_count, locals_count),
            "值 {} == locals_count 应不可重写（半开区间 [0, {})）",
            locals_count,
            locals_count
        );

        // 错误条件：典型的 ConstIdx 场景
        // 假设函数有 8 个寄存器，常量池索引为 100，远大于 locals_count
        // 如果 Opcode::operands() 把 ConstIdx 误标为 Reg，范围检查应阻止重写
        let small_locals: u16 = 8;
        let const_idx_value: u16 = 100;
        assert!(
            !is_remappable_reg(const_idx_value, small_locals),
            "ConstIdx 数值 {} 远大于 locals_count {}，应被范围检查阻止重写（防止误改常量池索引）",
            const_idx_value,
            small_locals
        );

        // 边界：locals_count = 0（空 chunk）
        assert!(!is_remappable_reg(0, 0), "locals_count=0 时任何值都不可重写");
        assert!(!is_remappable_reg(u16::MAX, 0), "locals_count=0 时 u16::MAX 也不可重写");

        // 边界：locals_count = u16::MAX
        assert!(is_remappable_reg(0, u16::MAX), "locals_count=u16::MAX 时值 0 可重写");
        assert!(
            is_remappable_reg(u16::MAX - 1, u16::MAX),
            "locals_count=u16::MAX 时 u16::MAX-1 可重写"
        );
        assert!(
            !is_remappable_reg(u16::MAX, u16::MAX),
            "locals_count=u16::MAX 时 u16::MAX 不可重写（半开区间）"
        );
    }

    /// 端到端校验：生成的 chunk 中所有指令的 operand 布局可被 `decode_operand_fields` 解析
    ///
    /// 构造一个包含 LoadK + Add + Return 的简单函数，验证生成后的字节码中
    /// 每条指令都能被 `decode_operand_fields` 正确解析（即 1 + operands_sum == instr_size
    /// 对定长指令成立）。这确保未来启用 LSRA rewrite 时不会因布局不一致而错位。
    #[test]
    fn test_codegen_instruction_encoding_assertion() {
        let mut module = IrModule::new();
        module.add_function("encode_check");
        let func = module.current_function_mut();
        let block = func.current_block_mut();

        // v0 = 10, v1 = 20, v2 = v0 + v1, return v2
        block.push(IrOp::LoadConstant { dest: ValueRef(0), constant: IrConstant::Number(10.0) });
        block.push(IrOp::LoadConstant { dest: ValueRef(1), constant: IrConstant::Number(20.0) });
        block.push(IrOp::Binary {
            dest: ValueRef(2),
            op: IrBinOp::Add,
            left: ValueRef(0),
            right: ValueRef(1),
        });
        block.push(IrOp::Return { value: Some(ValueRef(2)) });

        let mut codegen = CodeGenerator::new();
        let chunk = codegen.generate(&module).expect("encode_check codegen 应成功");

        // 遍历字节码，对每条指令验证 decode_operand_fields 返回字段的偏移 + 字节宽度
        // 之和与 instruction_size 一致（即定长指令布局自洽）
        let code = chunk.code();
        let mut ip: usize = 0;
        let mut visited_instr_count: usize = 0;
        while ip < code.len() {
            let opcode_byte = code[ip];
            let opcode = Opcode::decode_opcode(opcode_byte).expect("应能解码所有 opcode");

            let instr_size = opcode.instruction_size();
            let fields = decode_operand_fields(opcode);

            // 对定长指令：fields 总宽度应等于 instr_size - 1
            if !fields.is_empty() {
                let fields_sum: usize = fields.iter().map(|f| f.kind.byte_size()).sum();
                assert_eq!(
                    1 + fields_sum,
                    instr_size,
                    "IP={}: 指令 {:?} 的字段总宽度 {} + 1 != instruction_size {}",
                    ip,
                    opcode,
                    fields_sum,
                    instr_size
                );

                // 验证字段偏移连续且在指令范围内
                let mut expected_offset: usize = 1;
                for f in &fields {
                    assert_eq!(
                        f.offset, expected_offset,
                        "IP={}: 指令 {:?} 字段偏移错位，期望 {} 实际 {}",
                        ip, opcode, expected_offset, f.offset
                    );
                    expected_offset += f.kind.byte_size();
                }
                assert_eq!(
                    expected_offset,
                    1 + fields_sum,
                    "IP={}: 指令 {:?} 字段偏移累加与 fields_sum 不一致",
                    ip,
                    opcode
                );
            } else {
                // fields 为空有两种情况：
                //   a) 无操作数指令（如 Halt，operands=[] 且 1+0==instr_size）：跳过
                //   b) 变长指令（operands 非空但 1+operands_sum != instr_size）：
                //      验证 operands 布局确实与 instruction_size 不一致
                let operands = opcode.operands();
                if !operands.is_empty() {
                    let operands_sum: usize = operands.iter().map(|k| k.byte_size()).sum();
                    assert_ne!(
                        1 + operands_sum,
                        instr_size,
                        "IP={}: 指令 {:?} 返回空字段列表但 1 + operands_sum == instr_size，逻辑不一致",
                        ip,
                        opcode
                    );
                }
            }

            ip += instr_size;
            visited_instr_count += 1;
        }

        // 应至少访问了 LoadK + LoadK + Add + Return 4 条指令
        assert!(visited_instr_count >= 4, "应至少访问 4 条指令，实际 {}", visited_instr_count);
    }

    // ========================================================================
    // 回归测试：BUG-u8-arity-overflow（函数参数/捕获数量超 u8 范围）
    //
    // 触发场景：源代码定义 >255 个参数或 >255 个捕获变量的函数（极罕见）。
    // 旧实现：narrow_u8 溢出返回 CodegenError::Generic，Phase 1 容错 continue，
    //         Phase 2 报 "IrOp::Closure 引用了未注册的函数 ID"（降级为 C0000）。
    // 修复：register_sub_function 前置检查 + Phase 1 结构化硬错误立即返回。
    // ========================================================================

    /// 辅助：构造一个 main + sub_fn 的 IrModule，sub_fn 带指定数量的参数和捕获
    fn make_module_with_sub_fn(param_count: usize, capture_count: usize) -> IrModule {
        let mut module = IrModule::new();
        // main 函数（id=0）：return nil
        module.add_function("main");
        let main_func = module.current_function_mut();
        let main_block = main_func.current_block_mut();
        main_block.push(IrOp::LoadConstant { dest: ValueRef(0), constant: IrConstant::Nil });
        main_block.push(IrOp::Return { value: Some(ValueRef(0)) });

        // sub 函数（id=1）：带 N 个参数 + M 个捕获，body 只 return nil
        module.add_function("sub_fn");
        let sub_func = module.current_function_mut();
        sub_func.params = (0..param_count).map(|i| Arc::<str>::from(format!("p{}", i))).collect();
        sub_func.captures = (0..capture_count)
            .map(|i| nuzo_ir::types::CaptureDesc {
                name: Arc::<str>::from(format!("c{}", i)),
                is_mutable: false,
            })
            .collect();
        let sub_block = sub_func.current_block_mut();
        sub_block.push(IrOp::LoadConstant { dest: ValueRef(0), constant: IrConstant::Nil });
        sub_block.push(IrOp::Return { value: Some(ValueRef(0)) });
        module
    }

    #[test]
    fn test_too_many_params_256_returns_structured_error() {
        let mut codegen = CodeGenerator::new();
        let module = make_module_with_sub_fn(256, 0);
        let result = codegen.generate(&module);
        assert!(result.is_err(), "256 参数应编译失败");
        match result.unwrap_err() {
            CodegenError::TooManyParameters { count, max } => {
                assert_eq!(count, 256, "count 应为实际参数数");
                assert_eq!(max, 255, "max 应为 u8::MAX");
            }
            other => panic!("期望 TooManyParameters，实际 {:?}", other),
        }
    }

    #[test]
    fn test_too_many_params_255_ok() {
        let mut codegen = CodeGenerator::new();
        let module = make_module_with_sub_fn(255, 0);
        let result = codegen.generate(&module);
        assert!(result.is_ok(), "255 参数（u8::MAX）应编译成功，错误: {:?}", result.err());
    }

    #[test]
    fn test_too_many_captures_256_returns_structured_error() {
        let mut codegen = CodeGenerator::new();
        let module = make_module_with_sub_fn(0, 256);
        let result = codegen.generate(&module);
        assert!(result.is_err(), "256 捕获应编译失败");
        match result.unwrap_err() {
            CodegenError::TooManyCaptures { count, max } => {
                assert_eq!(count, 256, "count 应为实际捕获数");
                assert_eq!(max, 255, "max 应为 u8::MAX");
            }
            other => panic!("期望 TooManyCaptures，实际 {:?}", other),
        }
    }
}
