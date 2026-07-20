//! # 跳转回填 — 控制流跳转地址修复
//!
//! 负责 if/while/loop/for 等控制流的跳转目标地址回填。
//!
//! ## 核心机制
//!
//! 编译器在生成条件跳转和无条件跳转时，跳转目标往往尚未确定（前向引用）。
//! 此时先发射占位跳转指令，记录其 IP 地址，等到目标位置确定后再回填偏移量。
//!
//! ## 公开 API（Compiler 的 impl 方法）
//!
//! - `patch_jump(jump_ip, target_ip)` — 回填跳转指令的目标偏移量

use crate::compiler::{CompileError, Compiler};
use nuzo_bytecode::{Chunk, Opcode};

impl Compiler {
    // ========================================================================
    // 跳转回填（Jump Patching）
    // ========================================================================

    /// Patch a jump instruction's target offset
    ///
    /// This is used for forward jumps where the target isn't known at emit time.
    ///
    /// # 工作流程
    ///
    /// 1. 读取 jump_ip 处的 opcode 字节
    /// 2. 解码 Opcode 以确定指令大小（instruction_size）
    /// 3. 计算相对偏移量：target_ip - (jump_ip + instruction_size)
    /// 4. 检查偏移量是否在 i16 范围内
    /// 5. 根据 Opcode 类型确定偏移量的写入位置：
    ///    - Test 指令：偏移量在 jump_ip + 3 处（Test 有额外操作数）
    ///    - 其他指令：偏移量在 jump_ip + 1 处
    /// 6. 以小端序格式写入 i16 偏移量
    ///
    /// # 错误处理
    ///
    /// - `InvalidJumpTarget`：jump_ip 超出字节码有效范围
    /// - `InvalidOpcode`：jump_ip 处的 opcode 字节无法解码
    /// - `JumpOffsetOverflow`：计算出的偏移量超出 i16 范围
    /// - 通用 `Error`：字节码长度不足，无法完成回填
    pub(crate) fn patch_jump(
        &mut self,
        jump_ip: usize,
        target_ip: usize,
    ) -> Result<(), CompileError> {
        let opcode_byte =
            self.chunk.code().get(jump_ip).copied().ok_or(CompileError::InvalidJumpTarget {
                ip: jump_ip,
                line: self.current_line,
                column: self.current_column,
            })?;

        let decoded_op = Chunk::decode_opcode(opcode_byte).ok_or(CompileError::InvalidOpcode {
            ip: jump_ip,
            byte: opcode_byte,
            line: self.current_line,
            column: self.current_column,
        })?;
        let instr_size = decoded_op.instruction_size() as i32;

        let offset = target_ip as i32 - (jump_ip as i32 + instr_size);
        if offset > i16::MAX as i32 || offset < i16::MIN as i32 {
            return Err(CompileError::JumpOffsetOverflow {
                offset,
                from_ip: jump_ip,
                to_ip: target_ip,
                line: self.current_line,
                column: self.current_column,
            });
        }

        let offset_bytes = (offset as i16).to_le_bytes();

        let i16_start = match decoded_op {
            Opcode::Test => jump_ip + 3,
            _ => jump_ip + 1,
        };

        if i16_start + 1 < self.chunk.code().len() {
            self.chunk.code_mut()[i16_start] = offset_bytes[0];
            self.chunk.code_mut()[i16_start + 1] = offset_bytes[1];
        } else {
            return Err(CompileError::Error {
                message: format!("Cannot patch jump at ip {}: code too short", jump_ip),
                line: self.current_line,
                column: self.current_column,
            });
        }

        Ok(())
    }
}
