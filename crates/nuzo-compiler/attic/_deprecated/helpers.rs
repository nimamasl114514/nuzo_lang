//! # 编译器辅助工具模块（Bytecode Emission Utilities）
//!
//! 本模块提供编译器的**字节码发射基础设施**，是将高层操作转换为
//! 原始字节码序列的核心层，是编译器与虚拟机之间的**桥梁**。
//!
//! ## 核心发射函数
//!
//! | 方法 | 功能 |
//! |------|------|
//! | [`emit_opcode`](Compiler::emit_opcode) | 发射单个 Opcode 字节 + 调试信息 |
//! | [`emit_opcode_with_line`](Compiler::emit_opcode_with_line) | 发射 Opcode（显式行号版本） |
//! | [`emit_byte`](Compiler::emit_byte) | 发射原始 u8 字节 |
//! | [`emit_u16`](Compiler::emit_u16) | 发射小端序 u16 值（寄存器索引、常量池索引等） |
//! | [`emit_i16`](Compiler::emit_i16) | 发射小端序 i16 值（跳转偏移量） |
//! | [`emit_mov`](Compiler::emit_mov) | 寄存器到寄存器拷贝（含冗余消除 + LSRA Use 点记录） |
//!
//! ## 已迁移至子模块的方法
//!
//! 以下方法已从本模块提取到专用子模块，请查阅对应文件：
//!
//! | 方法类别 | 目标模块 |
//! |----------|----------|
//! | 常量池管理 (`add_constant_checked`) | [`string_intern`] |
//! | 跳转回填 (`patch_jump`) | [`patch_list`] |
//! | 作用域管理 (`begin_scope`/`end_scope`) | [`scope_management`] |
//! | 变量声明/查找 (`declare_local`/`find_local`) | [`scope_management`] |
//! | 寄存器分配/释放 (`alloc_register`/`release_registers` 等) | [`scope_management`] |

use crate::compiler::Compiler;
use nuzo_bytecode::Opcode;

impl Compiler {
    // ========================================================================
    // Bytecode Emission Helpers
    // ========================================================================

    /// Emit opcode with debug info (line + column)
    pub(super) fn emit_opcode(&mut self, op: Opcode, line: usize) {
        let ip = self.chunk.code().len();
        self.chunk.write_opcode(op);
        self.chunk.add_debug_info(ip, line, self.current_column);
    }

    /// Emit opcode with explicit line and column number
    ///
    /// Prefer this when the caller has a precise Span (line, column) from an AST node.
    /// Falls back to `self.current_column` when column is not available.
    pub(super) fn emit_opcode_with_line(&mut self, op: Opcode, line: usize) {
        self.emit_opcode(op, line);
    }

    /// Emit raw byte
    pub(super) fn emit_byte(&mut self, b: u8) {
        self.chunk.write_byte(b);
    }

    /// Emit u16 (little-endian)
    pub(super) fn emit_u16(&mut self, val: u16) {
        self.chunk.write_u16(val);
    }

    /// Emit i16 (little-endian)
    pub(super) fn emit_i16(&mut self, val: i16) {
        self.chunk.write_i16(val);
    }

    /// Emit MOV instruction (register-to-register copy)
    ///
    /// Redundant Mov elimination: skips emission when dest == src to avoid
    /// generating meaningless self-copy instructions.
    ///
    /// ## LSRA Integration (Use-point recording)
    ///
    /// When LSRA mode is enabled, automatically marks the src register as
    /// "in use at current IP" since Mov reads the src register value.
    pub(super) fn emit_mov(&mut self, dest: u16, src: u16) {
        if dest == src {
            return;
        }
        // LSRA: record src register use point (Mov reads src value)
        self.note_vreg_use(src);
        self.emit_opcode(Opcode::Mov, 0);
        self.emit_u16(dest);
        self.emit_u16(src);
    }
}
