//! 文档同步基础设施
//!
//! 提供 Opcode 文档的结构化提取，供 build.rs 生成 markdown 文档表。
//!
//! ## 设计目的
//!
//! `define_opcodes!` 宏在展开时会同步生成 `OPCODE_DOCS` 常量，
//! 汇总所有 Opcode 的名称、码值、操作数、描述与摘要。
//! 下游 crate（如 nuzo_bytecode）的 build.rs 可读取该常量，
//! 自动生成 markdown 文档表，避免人工维护文档与代码漂移。
//!
//! ## 跨 crate 依赖说明
//!
//! `OperandKind` 类型定义在 `nuzo_bytecode` 中，但 `nuzo_proc_core`
//! 不能反向依赖 `nuzo_bytecode`（会形成循环依赖）。
//! 因此 `OpcodeDoc::operands` 字段使用 `&'static str`（逗号分隔的字符串表示），
//! 而非 `&'static [OperandKind]`。

/// 单条 Opcode 的文档信息。
///
/// 由 `define_opcodes!` 宏在编译期生成，汇总到 `OPCODE_DOCS` 常量。
#[derive(Debug, Clone, Copy)]
pub struct OpcodeDoc {
    /// Opcode 标识符名称（如 "Halt"、"Add"）。
    pub name: &'static str,
    /// Opcode 数值码（0..=255）。
    pub code: u8,
    /// 操作数列表的字符串表示（逗号分隔，如 "Reg, Reg, Reg"）。
    ///
    /// 使用字符串而非 `&'static [OperandKind]` 是为了避免
    /// `nuzo_proc_core` 反向依赖 `nuzo_bytecode`。
    pub operands: &'static str,
    /// Opcode 的详细描述文本。
    pub desc: &'static str,
    /// 操作数摘要（用于文档表的一行说明）。
    pub summary: &'static str,
}
