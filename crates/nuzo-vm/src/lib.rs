//! # Nuzo VM — Nuzo 寄存机字节码解释器
//!
//! **层级**: L5（虚拟机层）—— 加载并执行字节码，提供寄存机解释器、垃圾回收器、对象系统与热路径优化。
//!
//! **主要入口**: [`VM`], [`VmObserver`], [`Gc`], [`Trace`], [`Object`], [`Shape`], [`ElasticRegisterFile`]
//!
//! ## 模块职责
//!
//! | 模块 | 职责 | 入口类型 |
//! |------|------|----------|
//! | [`vm`] | VM struct 定义、构造器、主执行循环、热路径批量执行 | [`VM`](vm::VM) |
//! | [`vm::builtin_registration`] | Builtin 函数注册（从 BuiltinRegistry 绑定到全局作用域） | (内部) |
//! | [`vm::variable_ops`] | 变量/寄存器/全局变量存取、诊断模式、GC 根扫描 | (内部) |
//! | [`vm::call_dispatch`] | 帧管理、调用派发、指令 tracing/fetch/patch | (内部) |
//! | [`gc`] | 增量标记-清除 GC（Region-Bump / SoA / ERSA 划痕区） | [`Gc`](gc::Gc), [`Trace`](gc::Trace) |
//! | [`object`] | Shape-based 属性缓存系统 | [`Object`](object::Object), [`Shape`](object::Shape) |
//! | [`dispatch`] | 指令派发（match opcode，主路径） | (内部) |
//! | [`dispatch_table`] | 函数指针派发表（legacy 路径） | (内部) |
//! | [`elastic_register_file`] | 自适应扩容寄存器文件（ElasticRegisterFile） | [`ElasticRegisterFile`](elastic_register_file::ElasticRegisterFile) |
//! | [`frame_paging`] | 深递归帧换页到堆（FramePager） | [`FramePager`](frame_paging::FramePager) |
//! | [`cache`] | Shape / InlineCache / StringPool / BytecodeCache | [`CacheManager`](cache::CacheManager) |
//! | [`vm_hot_trace`] | 热路径 trace 批量执行 JIT | [`HotTraceTable`](vm_hot_trace::HotTraceTable) |
//! | [`vm_lic`] | 多级内联缓存调用系统 (MLIC) | [`CallSites`](vm_lic::CallSites) |
//! | [`tracer_state`] | 指令级执行追踪器 | [`TracerState`](tracer_state::TracerState) |
//! | [`trf`] | ZeroUnbox 类型化寄存器文件 (TypedRegFile) | [`TypedRegFile`](trf::TypedRegFile) |
//! | [`zero_unbox`] | Smi 快路径 + 类型推断工具函数 | (工具函数集) |
//! | [`arena`] | Region/Bump Arena 函数作用域级分配器 | (内部) |
//!
//! ## 开发者速查：常见任务 → 代码位置
//!
//! | 任务 | 位置 |
//! |------|------|
//! | "加新指令" | `dispatch.rs` + `bytecode/opcode.rs` |
//! | "改 GC 策略/阈值" | `gc.rs` + `core/constants.rs` |
//! | "改 builtin 调用" | `builtin_registration.rs:register_builtins()` → `helpers/builtins.rs: BuiltinRegistry` |
//! | "改变量/寄存器操作" | `variable_ops.rs` (get_global, register, push/pop 等) |
//! | "改帧管理/调用派发" | `call_dispatch.rs` (push_frame, pop_frame, call_value 等) |
//! | "改寄存器分配" | `compiler/allocator.rs` (非 VM 内部) |
//! | "性能优化派发热路径" | `trf.rs` (ZeroUnbox) 或 `dispatch_table.rs` |
//! | "改主循环/热路径" | `vm.rs` (run/run_inner/execute_hot_trace_batch) |

#![allow(clippy::result_large_err)]

// Crate 元数据——外层属性形式（`#![inner_attr]` 在 stable Rust 不稳定）
#[nuzo_proc::crate_meta(layer = 5, description = "虚拟机与 dispatch", entry_type = "VM")]
const _NUZO_CRATE_META_ANCHOR: () = ();

pub mod arena;
pub mod cache;
pub mod elastic_register_file;
pub mod frame_paging;
pub mod gc;
pub mod object;
pub mod prelude;
pub mod tracer_state;
pub mod trf;
pub mod vm;
pub mod vm_hot_trace;
pub mod vm_lic;
pub mod zero_unbox;

// ============================================================================
// 选择性公开 re-export（符号来源可追踪）
// ============================================================================

// --- vm: 主执行引擎 ---
pub use vm::{ExecutionContext, NoopVmObserver, VM, VmObserver};

// --- gc: 垃圾回收 ---
pub use gc::{ChunkInfo, GC_DID_COLLECT_KEY, GC_WILL_COLLECT_KEY, Gc, GcStats, Trace, is_scratch};

// --- object: 对象/Shape 系统 ---
pub use object::{Object, Shape};

// --- vm_hot_trace: 热路径追踪 + Superinstruction Fusion ---
pub use vm_hot_trace::{FusedLoopEntry, MicroOp};
