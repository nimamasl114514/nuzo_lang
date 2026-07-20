//! # 控制流 opcode 实现
//!
//! 包含跳转类指令：
//! - `op_jmp` — 无条件跳转
//! - `op_test` — 条件跳转（falsy 时跳转）

use crate::vm::VM;
use nuzo_values::NuzoError;

impl VM {
    #[inline(always)]
    pub(in crate::vm) fn op_jmp(&mut self) -> Result<(), NuzoError> {
        let offset = self.read_i16()?;
        self.ip = self.validate_jump_target(offset, false)?;
        Ok(())
    }

    pub(in crate::vm) fn op_test(&mut self) -> Result<(), NuzoError> {
        let reg = self.read_u16()?;
        let offset = self.read_i16()?;
        let test_val = self.register(reg)?;
        // H4 Bug Fix (BUG-003): Always validate the jump target for consistency with op_jmp.
        // Previously validation only ran when the branch was taken (falsy condition),
        // so out-of-bounds offsets were silently ignored when the condition was truthy.
        let new_ip = self.validate_jump_target(offset, true)?;
        if !nuzo_core::tag::is_truthy(test_val.into_raw_bits()) {
            self.ip = new_ip;
        }
        Ok(())
    }
}
