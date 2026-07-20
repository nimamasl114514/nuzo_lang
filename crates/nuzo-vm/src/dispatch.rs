// VM Opcode Dispatch — Module Entry
//
// 本文件是 `dispatch` 模块的入口，声明所有子模块并重导出公共类型。
// 实际 opcode 派发逻辑已按功能域拆分到 `dispatch/` 目录下的子模块中：
//
// - `cache_types` — 派发缓存类型（GMVC / PIC / CSTS / CDD）
// - `cold_path` — 冷路径辅助（错误构造 + 诊断 + trace）
// - `helpers` — VM 派发辅助方法（常量读取 / 模块加载 / Arena / 跳转校验 / Builtin 展开）
// - `control_flow` — 控制流 opcode（op_jmp / op_test）
// - `property` — PIC 属性访问 opcode（op_get_prop / op_set_prop）
// - `index_ops` — 索引访问 opcode（op_get_index / op_set_index / op_set_index_mut）
// - `calls` — 函数调用派发 + op_call 拆分 + op_return + op_halt
// - `tail_call` — TCO 实现 + execute_tail_call 拆分
// - `capture` — 闭包变量捕获 + op_capture 拆分 + op_get_captured / op_set_captured
// - `globals` — 全局变量 opcode（GMVC + ISS）
// - `heap_ops` — 堆对象构造与查询 opcode
//
// 性能优化版：CIGC, ZOS-IC, Unrolled Builtin Dispatch
// 论文级创新：PIC (多态属性缓存), CDD (闭包直接调度), GMVC (全局变量多版本缓存)

#[path = "dispatch/cache_types.rs"]
mod cache_types;
#[path = "dispatch/calls.rs"]
mod calls;
#[path = "dispatch/capture.rs"]
mod capture;
#[path = "dispatch/cold_path.rs"]
mod cold_path;
#[path = "dispatch/control_flow.rs"]
mod control_flow;
#[path = "dispatch/globals.rs"]
mod globals;
#[path = "dispatch/heap_ops.rs"]
mod heap_ops;
#[path = "dispatch/helpers.rs"]
mod helpers;
#[path = "dispatch/index_ops.rs"]
mod index_ops;
#[path = "dispatch/property.rs"]
mod property;
#[path = "dispatch/tail_call.rs"]
mod tail_call;

// 重导出：被 vm.rs / vm_lic.rs 等外部模块通过 `dispatch::TypeName` 路径访问
pub(crate) use cache_types::{ClosureInvoker, ClosureSnapshot, GlobalCacheEntry, PropICSlot};

use super::VM;
use super::dispatch_table::dispatch_opcode_fast;
use nuzo_bytecode::Opcode;
use nuzo_values::NuzoError;

impl VM {
    /// 执行单条 opcode，包含 trace 记录（冷路径）。
    pub fn execute(&mut self, opcode: Opcode) -> Result<(), NuzoError> {
        let ip_before = self.ip.saturating_sub(1);
        let trace_start =
            if self.tracer_should_record(&opcode) { Some(web_time::Instant::now()) } else { None };
        let execute_result = self.execute_inner(opcode);
        if let Some(start) = trace_start {
            self.handle_trace_cold(opcode, ip_before, start.elapsed());
        }
        execute_result
    }

    #[inline(always)]
    pub(crate) fn execute_inner(&mut self, opcode: Opcode) -> Result<(), NuzoError> {
        dispatch_opcode_fast(self, opcode)
    }
}
