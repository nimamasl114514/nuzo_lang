//! # nuzo_vm prelude
//!
//! 统一 re-export `nuzo_vm` crate 中最常用的类型，方便外部调用者通过
//! `use nuzo_vm::prelude::*;` 一键导入。
//!
//! ## 使用方式
//!
//! ```rust,ignore
//! use nuzo_vm::prelude::*;
//! ```
//!
//! ## 未包含的类型
//!
//! 以下类型在 crate 内部使用（`pub(super)`），不对外暴露，故不在 prelude 中：
//! - `FrameMeta` / `FrameInfo` — SCHF v6 帧栈结构，内部帧管理
//! - `ExceptionFrame` — 异常帧，内部异常处理
//! - `InlineCacheEntry` — 内联缓存条目，内部 dispatch 优化

// --- VM 执行引擎 ---
pub use crate::vm::{ExecutionContext, NoopVmObserver, VM, VmObserver};

// --- GC 垃圾回收 ---
pub use crate::gc::{ChunkInfo, Gc, GcStats, Trace, is_scratch};
pub use crate::gc::{GC_DID_COLLECT_KEY, GC_WILL_COLLECT_KEY};

// --- 对象/Shape 系统 ---
pub use crate::object::{Object, Shape};

// --- 自适应寄存器文件 ---
pub use crate::elastic_register_file::ElasticRegisterFile;

// --- 帧换页 ---
pub use crate::frame_paging::FramePagerStats;

// --- 缓存系统 ---
pub use crate::cache::{CacheGlobalStats, CacheManager};

// --- 热路径追踪 ---
pub use crate::vm_hot_trace::{
    FusedLoopEntry, HotTraceConfig, HotTraceEntry, HotTraceTable, MicroOp,
};

// --- 多级内联缓存 ---
pub use crate::vm_lic::{CacheHit, CallSite, CallSiteStats, CallSites};

// --- 类型化寄存器文件 ---
pub use crate::trf::TypedRegFile;

// --- 指令级追踪 ---
pub use crate::tracer_state::{TraceConfig, TraceEntry, TraceResult};
