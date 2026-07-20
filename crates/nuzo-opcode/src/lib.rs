//! # nuzo_opcode — Nuzo 声明式操作码定义框架
//!
//! **层级**: L2（语言核心层）—— 定义 VM 指令集的操作数类型、分发模式与声明式宏，是字节码与虚拟机的基础契约层。
//!
//! **主要入口**: [`define_opcodes!`], [`OperandKind`], [`DispatchKind`], [`DisasmStyle`]
//!
//! 本 crate 为 VM 指令集提供声明式的操作码（opcode）定义基础设施。
//! 核心设计理念：
//! - 用宏一次性定义指令集，自动生成反汇编、编解码等样板代码
//! - 零运行时外部依赖（编译期仅依赖 nuzo_proc proc-macro），可被任何 Nuzo 子 crate 引用
//! - 操作数类型（`OperandKind`）精确描述字节宽度与符号性
//!
//! ## 架构概览
//!
//! ### 设计哲学：数据驱动（Data-Driven）
//! 传统做法是为每条指令手写 `match` 分支，导致：
//! - **重复代码**：解码、反汇编、分发逻辑分散在多处
//! - **易出错**：新增指令时容易遗漏某个分支
//! - **维护成本高**：修改指令格式需同步多处
//!
//! 本框架采用 **"定义一次，到处生成"** 的策略：
//! ```ignore
//! // 仅在此处定义一次
//! define_opcodes! {
//!     Add = 0x01, size: 7, operands: [Reg, Reg, Reg], disasm: "{dst} = {lhs} + {rhs}";
//! }
//!
//! // 自动获得：
//! // - Opcode::Add 枚举变体
//! // - Add.instruction_size()  -> 7
//! // - Add.operands()          -> [Reg, Reg, Reg]
//! // - Add.dispatch_kind()     -> BinaryArithmetic
//! // - Opcode::decode_opcode(0x01) -> Some(Add)
//! // - 编译期大小校验 (1 + 2+2+2 == 7)
//! ```
//!
//! ## 核心类型体系
//!
//! ### 1. [`Opcode`] — 操作码枚举
//! 由 `define_opcodes!` 宏自动生成，每个变体代表一条 VM 指令。
//! 使用 `#[repr(u8)]` 保证内存布局紧凑（单字节）。
//!
//! ### 2. [`OperandKind`] — 操作数类型系统
//! 精确描述指令的操作数字段：
//!
//! | 类型 | 字节宽度 | 符号性 | 典型用途 |
//! |------|---------|--------|---------|
//! | `Reg` | 2 bytes | 无符号 | 寄存器索引（目标/源1/源2）|
//! | `Const` | 2 bytes | 无符号 | 常量池索引 |
//! | `Offset` | 2 bytes | **有符号** | 跳转偏移量（支持前向/后向跳转）|
//! | `U8` | 1 byte | 无标志 | 小立即数（如参数数量）|
//! | `U16` | 2 bytes | 无符号 | 大立即数（如 Upvalue 索引）|
//! | `CaptureIdx` | 2 bytes | 无符号 | 闭包捕获槽索引 |
//! | `None` | 0 bytes | — | 无操作数（如 Halt, Return）|
//!
//! ### 3. [`DispatchKind`] — 分发模式分类
//! 将指令按**执行逻辑类别**分组，实现表驱动分发：
//!
//! **加载类（Load Family）**:
//! - `LoadFromPool`: 从常量池加载（`LoadK`）
//! - `LoadConst`: 加载字面量常量（`LoadNil`, `LoadTrue`, `LoadFalse`, `LoadNumber`）
//!
//! **运算类（Arithmetic Family）**:
//! - `BinaryArithmetic`: 二元运算（`Add`, `Sub`, `Mul`, `Div`, `Mod`, `Pow`）
//! - `UnaryOp`: 一元运算（`Neg`, `BitNot`）
//! - `LogicalNot`: 逻辑非（`Not`）
//!
//! **比较类（Comparison Family）**:
//! - `EqualityComparison`: 等值比较（`Eq`, `Neq`）
//! - `BinaryComparison`: 有序比较（`Lt`, `Gt`, `Le`, `Ge`）
//!
//! **控制流类（Control Flow Family）**:
//! - `Custom`: 手动实现的复杂指令（`Jmp`, `Call`, `Return`, `Closure` 等）|
//!
//! ### 4. [`DisasmStyle`] — 反汇编输出风格
//! 控制反汇编器的显示格式：
//! - `Template(String)`: 使用模板字符串（如 `"{dst} = {src} + {imm}"`）|
//! - `Custom`: 使用自定义回调函数
//!
//! ## 编译期保证
//!
//! 宏会自动验证**声明的指令大小**是否与**实际操作数大小之和**匹配：
//! ```compile_fail
//! define_opcodes! {
//!     // 错误！声明 size=5，但实际是 1(opcode) + 2(Reg) + 2(Const) = 5 ✓
//!     LoadK = 1, size: 3, operands: [Reg, Const];  // 编译错误!
//! }
//! ```
//!
//! 这种**编译期校验**消除了整类运行时 bug。
//!
//! ## 指令集设计原则
//!
//! ### 1. 固定长度指令
//! 所有指令长度在编译时确定，无变长指令：
//! - **优点**：解码简单（PC += instruction_size()），无需扫描操作码
//! - **缺点**：某些简单指令浪费字节（如 `Halt` 占 1 字节合理，但 `Nop` 也占 1 字节）|
//!
//! ### 2. 三地址码（Three-Address Code）
//! 算术运算采用三地址形式：`OP dst, src1, src2`
//! - 避免破坏性操作（不覆盖源操作数）|
//! - 更容易进行 SSA 转换和优化
//! - 代价：指令较长（7 字节 vs 3 字节的栈式 VM）|
//!
//! ### 3. 基于寄存器（Register-Based）
//! 使用虚拟寄存器文件而非栈：
//! - 减少Push/Pop 开销
//!   支持更好的并行化潜力（未来 JIT 优化）
//! - 编译器负责寄存器分配（当前使用简单线性扫描）|
//!
//! ## 与其他 Crate 的关系
//!
//! ```text
//! nuzo_bytecode ──use──> nuzo_opcode (定义实际指令集)
//!      │
//!      ├── nuzo_vm (VM 实现，依赖 Opcode 进行分发)
//!      ├── nuzo_compiler (编译器，生成 Opcode 序列)
//!      └── nuzo_testkit (测试工具，断言 Opcode 正确性)
//! ```
//!
//! **关键点**：本 crate **仅提供宏和类型定义**，实际的 `define_opcodes!` 调用
//! 在 `nuzo_bytecode` 中完成，以避免循环依赖。
//!
//! ## 快速上手
//! ```ignore
//! use nuzo_opcode::{Opcode, OperandKind, DispatchKind};
//!
//! // 解码操作码
//! if let Some(op) = Opcode::decode_opcode(0x02) {
//!     println!("Instruction: {}", op.name());           // "Add"
//!     println!("Size: {} bytes", op.instruction_size());  // 7
//!     println!("Operands: {:?}", op.operands());          // [Reg, Reg, Reg]
//!     println!("Dispatch: {:?}", op.dispatch_kind());      // BinaryArithmetic
//! }
//!
//! // 查询操作数属性
//! assert_eq!(OperandKind::Offset.byte_size(), 2);
//! assert!(OperandKind::Offset.is_signed());
//! assert!(!OperandKind::Reg.is_signed());
//! ```

// Crate 元数据——外层属性形式（`#![inner_attr]` 在 stable Rust 不稳定）
#[nuzo_proc::crate_meta(
    layer = 2,
    description = "Opcode 框架与 define_opcodes 宏",
    entry_type = "Opcode"
)]
const _NUZO_CRATE_META_ANCHOR: () = ();

/// 描述 VM 指令中单个操作数的种类、字节宽度与符号性。
///
/// 这是类型系统的核心，用于：
/// 1. **编译期大小校验**：确保声明的 `size` 与实际操作数总宽度一致
/// 2. **解码器指导**：告诉解码器如何从字节流中提取操作数值
/// 3. **反汇编器格式化**：根据类型选择合适的显示格式（如有符号十进制/十六进制）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, nuzo_proc::MatchSync)]
pub enum OperandKind {
    /// 寄存器索引，2 字节无符号
    ///
    /// 用于指向 VM 寄存器文件中的某个寄存器（R0 ~ R65535）。
    /// 在三地址指令中通常表示：dst（目标）、src1（左操作数）、src2（右操作数）。
    Reg,

    /// 常量池索引，2 字节无符号
    ///
    /// 指向 Chunk 的常量池（constants array）中的条目索引。
    /// 编译器将所有字面量（数字、字符串、布尔值）收集到常量池中，
    /// 运行时通过此索引查找。
    Const,

    /// 有符号跳转偏移量，2 字节有符号
    ///
    /// 用于 `Jmp`, `JmpIfTrue`, `JmpIfFalse` 等控制流指令。
    /// **有符号**是因为需要同时支持前向跳转（正偏移）和后向跳转（负偏移，如循环回退）。
    /// 偏移量相对于**下一条指令的起始位置**计算（非当前指令）。
    Offset,

    /// 单字节无符号立即数，1 字节
    ///
    /// 用于小范围立即数，如：
    /// - `Call` 的参数数量（0~255 个参数足够覆盖绝大多数情况）|
    /// - `BuildList` 的初始元素数量
    U8,

    /// 双字节无符号立即数，2 字节
    ///
    /// 用于较大范围的立即数，如：
    /// - Upvalue 捕获列表长度
    /// - 对象属性数量
    U16,

    /// 闭包捕获槽索引，2 字节无符号（语义不同于寄存器）
    ///
    /// 虽然字节宽度和 `Reg` 相同，但语义不同：
    /// - `Reg`：指向当前函数帧的局部变量/临时变量
    /// - `CaptureIdx`：指向闭包对象的捕获变量数组（存储外层函数的局部变量副本）|
    ///
    /// 分开定义有助于静态分析阶段区分这两种访问模式。
    CaptureIdx,

    /// 4 字节无符号立即数 (0..=4_294_967_295)
    ///
    /// 用于 ISS (Instruction Self-Specialization) 特化指令的缓存数据：
    /// - 全局变量索引 (`gidx`)
    /// - 全局变量版本号 (`version`)
    /// - 形状守卫 (`shape_guard`)
    U32,

    /// 无操作数，0 字节
    ///
    /// 用于无操作数的指令，如：
    /// - `Halt`: VM 停机
    /// - `Return`: 从函数返回（返回值隐式约定为 R0）|
    /// - `Nil`: 加载空值到 R0
    None,
}

impl OperandKind {
    /// 2 字节操作数宽度（Reg / Const / Offset / U16 / CaptureIdx）。
    const WORD_SIZE: usize = 2;
    /// 1 字节操作数宽度（U8）。
    const BYTE_SIZE: usize = 1;
    /// 4 字节操作数宽度（U32）。
    const DWORD_SIZE: usize = 4;
    /// 无操作数宽度（None）。
    const ZERO_SIZE: usize = 0;

    /// 返回该操作数在字节码中占用的字节数。
    #[inline]
    pub const fn byte_size(&self) -> usize {
        match self {
            OperandKind::Reg => Self::WORD_SIZE,
            OperandKind::Const => Self::WORD_SIZE,
            OperandKind::Offset => Self::WORD_SIZE,
            OperandKind::U8 => Self::BYTE_SIZE,
            OperandKind::U16 => Self::WORD_SIZE,
            OperandKind::CaptureIdx => Self::WORD_SIZE,
            OperandKind::U32 => Self::DWORD_SIZE,
            OperandKind::None => Self::ZERO_SIZE,
        }
    }

    /// 返回该操作数是否为有符号类型。
    ///
    /// 目前仅 `Offset` 为有符号；其余均为无符号。
    #[inline]
    pub const fn is_signed(&self) -> bool {
        matches!(self, OperandKind::Offset)
    }
}

/// 控制反汇编时的输出格式。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum DisasmStyle {
    /// 使用模板字符串格式化，例如 `"{dst} = {src} + {imm}"`。
    Template(String),
    /// 使用自定义回调 / 手动格式化逻辑。
    Custom,
}

/// 描述 VM 指令的分发模式。
///
/// 每个操作码对应一种分发模式，用于在 VM 主循环中确定执行逻辑的类别。
/// 这使得 dispatch 可以基于类别进行表驱动分发，而非逐条 match。
#[derive(Debug, Clone, Copy, PartialEq, Eq, nuzo_proc::MatchSync)]
pub enum DispatchKind {
    /// 从常量池加载（LoadK）
    LoadFromPool,
    /// 加载常量字面量（LoadNil, LoadTrue, LoadFalse）
    LoadConst,
    /// 寄存器复制（Mov）
    MovRegister,
    /// 二元算术运算（Add, Sub, Mul, Div, Rem, Mod, Pow）
    BinaryArithmetic,
    /// 一元操作（Neg）
    UnaryOp,
    /// 逻辑非（Not）
    LogicalNot,
    /// 等值比较（Eq, Neq）
    EqualityComparison,
    /// 二元比较运算（Lt, Gt, Le, Ge）
    BinaryComparison,
    /// 打印值（Print）
    PrintValue,
    /// 加载闭包原型（Closure）
    LoadClosure,
    /// 手动实现（Jmp, Test, Call, Return, 等）
    Custom,
    /// ISS 特化全局变量读取（GetGlobalCached）
    GetGlobalCached,
}

//
// 已迁移至 `nuzo_proc::define_opcodes!` proc-macro。
// 新 API 使用属性语法：
//
// ```ignore
// nuzo_proc::define_opcodes! {
//     /// 停机指令
//     #[opcode(code = 0, size = 1, operands = [], disasm = "halt")]
//     Halt,
//
//     /// 加载常量到寄存器
//     #[opcode(code = 1, size = 5, operands = [Reg, Const], disasm = custom)]
//     LoadK,
// }
// ```
//
// 本处 re-export 以保持向后兼容的 `use nuzo_opcode::define_opcodes;` 路径。

pub use nuzo_proc::define_opcodes;

//
// define_opcodes! 宏仅在此定义，不在本 crate 中调用。
// 实际的指令集定义由 nuzo_bytecode 通过 `use nuzo_opcode::define_opcodes;`
// 调用本宏来完成，这样 nuzo_bytecode 可以获得完整的 Opcode 枚举及其方法。

#[cfg(test)]
mod tests {
    use super::*;

    nuzo_proc::define_opcodes! {
        /// 测试用：停机指令
        #[opcode(code = 0, size = 1, operands = [], disasm = "halt")]
        Halt,

        /// 测试用：加载常量
        #[opcode(code = 1, size = 5, operands = [Reg, Const], disasm = custom)]
        LoadK,

        /// 测试用：加法
        #[opcode(code = 2, size = 7, operands = [Reg, Reg, Reg], disasm = custom)]
        Add,

        /// 测试用：跳转
        #[opcode(code = 3, size = 3, operands = [Offset], disasm = custom)]
        Jmp,
    }

    #[test]
    fn halt_opcode_value() {
        assert_eq!(Opcode::Halt as u8, 0);
    }

    #[test]
    fn halt_instruction_size() {
        assert_eq!(Opcode::Halt.instruction_size(), 1);
    }

    #[test]
    fn halt_name() {
        assert_eq!(Opcode::Halt.name(), "Halt");
    }

    #[test]
    fn decode_halt() {
        assert_eq!(Opcode::decode_opcode(0), Some(Opcode::Halt));
    }

    #[test]
    fn add_instruction_size() {
        assert_eq!(Opcode::Add.instruction_size(), 7);
    }

    #[test]
    fn jmp_operands() {
        assert_eq!(Opcode::Jmp.operands(), &[OperandKind::Offset]);
    }

    #[test]
    fn halt_disasm_template() {
        assert_eq!(Opcode::Halt.disasm_template(), Some("halt"));
    }

    #[test]
    fn loadk_disasm_template_is_custom() {
        assert_eq!(Opcode::LoadK.disasm_template(), None);
    }

    #[test]
    fn display_implementation() {
        assert_eq!(format!("{}", Opcode::Halt), "Halt");
        assert_eq!(format!("{}", Opcode::Add), "Add");
    }

    #[test]
    fn decode_invalid_opcode() {
        assert_eq!(Opcode::decode_opcode(200), None);
    }

    #[test]
    fn loadk_operands() {
        assert_eq!(Opcode::LoadK.operands(), &[OperandKind::Reg, OperandKind::Const]);
    }

    #[test]
    fn halt_operands_empty() {
        assert_eq!(Opcode::Halt.operands(), &[]);
    }

    #[test]
    fn all_opcodes_decode_roundtrip() {
        for i in 0u8..=3 {
            let op = Opcode::decode_opcode(i).expect("opcodes 0..=3 must all decode");
            assert_eq!(op as u8, i, "roundtrip failed for opcode {i}");
        }
    }

    #[test]
    fn serde_serialize_works() {
        // 仅在 serde feature 启用时测试
        #[cfg(feature = "serde")]
        {
            let json = serde_json::to_string(&Opcode::Halt).unwrap();
            assert!(json.contains("Halt") || json.contains("0"), "serde serialize should work");
        }
    }

    #[test]
    fn old_form_dispatch_defaults_to_custom() {
        assert_eq!(Opcode::Halt.dispatch_kind(), DispatchKind::Custom);
        assert_eq!(Opcode::LoadK.dispatch_kind(), DispatchKind::Custom);
        assert_eq!(Opcode::Add.dispatch_kind(), DispatchKind::Custom);
        assert_eq!(Opcode::Jmp.dispatch_kind(), DispatchKind::Custom);
    }
}

// ── 新形式 dispatch 测试（独立模块，避免 Opcode 冲突）──────────────

#[cfg(test)]
mod dispatch_tests {
    use crate::{DispatchKind, OperandKind};

    nuzo_proc::define_opcodes! {
        /// 测试用：停机指令
        #[opcode(code = 0, size = 1, operands = [], disasm = "halt", dispatch = Custom, desc = "测试用：停机指令", summary = "")]
        TestHalt,

        /// 测试用：加载常量
        #[opcode(code = 1, size = 5, operands = [Reg, Const], disasm = custom, dispatch = LoadFromPool, desc = "测试用：加载常量", summary = "dest, const_idx")]
        TestLoadK,

        /// 测试用：加法
        #[opcode(code = 2, size = 7, operands = [Reg, Reg, Reg], disasm = custom, dispatch = BinaryArithmetic, desc = "测试用：加法", summary = "dest, left, right")]
        TestAdd,

        /// 测试用：小于比较
        #[opcode(code = 3, size = 7, operands = [Reg, Reg, Reg], disasm = custom, dispatch = BinaryComparison, desc = "测试用：小于比较", summary = "dest, left, right")]
        TestLt,

        /// 测试用：相等比较
        #[opcode(code = 4, size = 7, operands = [Reg, Reg, Reg], disasm = custom, dispatch = EqualityComparison, desc = "测试用：相等比较", summary = "dest, left, right")]
        TestEq,

        /// 测试用：取负
        #[opcode(code = 5, size = 5, operands = [Reg, Reg], disasm = custom, dispatch = UnaryOp, desc = "测试用：取负", summary = "dest, src")]
        TestNeg,

        /// 测试用：加载 Nil
        #[opcode(code = 6, size = 3, operands = [Reg], disasm = custom, dispatch = LoadConst, desc = "测试用：加载 Nil", summary = "dest")]
        TestLoadNil,

        /// 测试用：寄存器移动
        #[opcode(code = 7, size = 5, operands = [Reg, Reg], disasm = custom, dispatch = MovRegister, desc = "测试用：寄存器移动", summary = "dest, src")]
        TestMov,

        /// 测试用：逻辑非
        #[opcode(code = 8, size = 5, operands = [Reg, Reg], disasm = custom, dispatch = LogicalNot, desc = "测试用：逻辑非", summary = "dest, src")]
        TestNot,

        /// 测试用：打印
        #[opcode(code = 9, size = 3, operands = [Reg], disasm = custom, dispatch = PrintValue, desc = "测试用：打印", summary = "reg")]
        TestPrint,

        /// 测试用：创建闭包
        #[opcode(code = 10, size = 5, operands = [Reg, Const], disasm = custom, dispatch = LoadClosure, desc = "测试用：创建闭包", summary = "dest, proto")]
        TestClosure,
    }

    /// 表驱动的 dispatch_kind() 测试：新增 dispatch 类型时只需追加一行。
    #[test]
    fn dispatch_kind_table_driven() {
        let cases: &[(Opcode, DispatchKind)] = &[
            (Opcode::TestHalt, DispatchKind::Custom),
            (Opcode::TestLoadK, DispatchKind::LoadFromPool),
            (Opcode::TestAdd, DispatchKind::BinaryArithmetic),
            (Opcode::TestLt, DispatchKind::BinaryComparison),
            (Opcode::TestEq, DispatchKind::EqualityComparison),
            (Opcode::TestNeg, DispatchKind::UnaryOp),
            (Opcode::TestLoadNil, DispatchKind::LoadConst),
            (Opcode::TestMov, DispatchKind::MovRegister),
            (Opcode::TestNot, DispatchKind::LogicalNot),
            (Opcode::TestPrint, DispatchKind::PrintValue),
            (Opcode::TestClosure, DispatchKind::LoadClosure),
        ];

        for &(opcode, expected) in cases {
            assert_eq!(opcode.dispatch_kind(), expected, "dispatch_kind mismatch for {:?}", opcode,);
        }
    }
}

#[cfg(test)]
mod match_sync_demo {
    use crate::{MatchSyncOperandKind, OperandKind};

    /// 集中式处理器：所有 OperandKind 的 byte_size 逻辑收敛到此处。
    ///
    /// 当 OperandKind 新增变体时，编译器会强制要求在此处添加对应的 on_xxx 方法，
    /// 所有调用 `match_sync` 的地方自动同步，无需逐个文件修改。
    struct ByteSizeHandler;

    impl MatchSyncOperandKind<usize> for ByteSizeHandler {
        fn on_reg(&self) -> usize {
            2
        }
        fn on_const(&self) -> usize {
            2
        }
        fn on_offset(&self) -> usize {
            2
        }
        fn on_u8(&self) -> usize {
            1
        }
        fn on_u16(&self) -> usize {
            2
        }
        fn on_capture_idx(&self) -> usize {
            2
        }
        fn on_u32(&self) -> usize {
            4
        }
        fn on_none(&self) -> usize {
            0
        }
    }

    #[test]
    fn match_sync_byte_size() {
        let handler = ByteSizeHandler;
        assert_eq!(OperandKind::Reg.match_sync(&handler), 2);
        assert_eq!(OperandKind::Const.match_sync(&handler), 2);
        assert_eq!(OperandKind::Offset.match_sync(&handler), 2);
        assert_eq!(OperandKind::U8.match_sync(&handler), 1);
        assert_eq!(OperandKind::U16.match_sync(&handler), 2);
        assert_eq!(OperandKind::CaptureIdx.match_sync(&handler), 2);
        assert_eq!(OperandKind::U32.match_sync(&handler), 4);
        assert_eq!(OperandKind::None.match_sync(&handler), 0);
    }
}
