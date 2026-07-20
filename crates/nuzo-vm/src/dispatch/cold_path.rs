//! # 冷路径辅助函数
//!
//! 集中 VM 派发中的冷路径代码：
//! - 错误构造辅助（`err_*` 系列函数，全部 `#[cold]` + `#[inline(never)]`）
//! - 诊断信息构造
//! - 指令 trace 冷路径处理

use crate::vm::VM;
use nuzo_bytecode::Opcode;
use nuzo_core::DIAGNOSTIC_REGISTER_WINDOW;
use nuzo_values::{InternalError, NuzoError, VmDiagnosis};

// ========================================================================
// 🧊 Cold Path Helpers
// ========================================================================

#[cold]
#[inline(never)]
pub(super) fn err_compiler_bug(msg: &str, diagnosis: Option<VmDiagnosis>) -> NuzoError {
    NuzoError::internal(InternalError::CompilerBug { message: msg.to_string() }, diagnosis)
}

#[cold]
#[inline(never)]
pub(super) fn err_const_out_of_bounds(
    idx: usize,
    len: usize,
    _ip: usize,
    diagnosis: Option<VmDiagnosis>,
) -> NuzoError {
    NuzoError::internal(
        InternalError::ConstantOutOfBounds { index: idx, pool_size: len },
        diagnosis,
    )
}

#[cold]
#[inline(never)]
pub(super) fn err_stack_overflow(depth: usize, max: usize, is_tco: bool) -> NuzoError {
    let hint = if is_tco {
        "Tail call stack overflow. TCO is active but register demand exceeds capacity."
    } else {
        "Normal call stack overflow. Consider refactoring to use tail calls."
    };
    NuzoError::internal(
        InternalError::StackOverflow { depth, max_depth: max },
        Some(VmDiagnosis {
            disassembly: format!("Stack overflow at depth {}/{}.", depth, max),
            error_ip: None,
            register_snapshot: vec![],
            call_stack_depth: 0,
            root_cause_analysis: hint.to_string(),
        }),
    )
}

impl VM {
    // ========================================================================
    // Helper Methods
    // ========================================================================

    #[cold]
    #[inline(never)]
    pub(super) fn current_diagnosis(&self, message: &str) -> VmDiagnosis {
        let start = self.cx.registers.len().saturating_sub(DIAGNOSTIC_REGISTER_WINDOW);
        let register_snapshot: Vec<(u16, String)> = self
            .cx
            .registers
            .as_slice()
            .iter()
            .enumerate()
            .skip(start)
            .map(|(i, v)| (i as u16, format!("{}", v)))
            .collect();

        VmDiagnosis {
            disassembly: message.to_string(),
            error_ip: Some(self.ip),
            register_snapshot,
            call_stack_depth: self.frame_depth(),
            root_cause_analysis: format!("Internal error at IP {}: {}", self.ip, message),
        }
    }

    #[cold]
    #[inline(never)]
    pub(super) fn handle_trace_cold(
        &mut self,
        opcode: Opcode,
        ip_before: usize,
        duration: std::time::Duration,
    ) {
        let registers_before = self.capture_registers_for_trace();
        self.record_trace(opcode, vec![], ip_before, duration.as_nanos(), registers_before);
    }
}
