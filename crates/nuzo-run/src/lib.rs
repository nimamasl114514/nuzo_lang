//! # nuzo_run — Nuzo 语言统一应用入口
//!
//! **层级**: L6（应用层）—— 整合引擎、会话、CLI、配置、插件与测试框架，为最终用户提供运行、调试、REPL、基准测试与插件扩展的单一门面。
//!
//! **主要入口**: [`Engine`], [`EngineBuilder`], [`Session`], [`NuzoPlugin`], [`BenchHarness`], [`TestHarness`]
//!
//! 提供 Engine（长生命周期配置）+ Session（单次执行上下文），以及 CLI、基准测试和插件基础设施。

#![allow(clippy::result_large_err)]

// Crate 元数据——外层属性形式（`#![inner_attr]` 在 stable Rust 不稳定）
#[nuzo_proc::crate_meta(layer = 6, description = "CLI 入口与会话管理", entry_type = "Engine")]
const _NUZO_CRATE_META_ANCHOR: () = ();

mod bench;
mod config;
mod engine;
mod error;
mod output;
pub mod output_sink;
mod plugin;
pub mod prelude;
mod session;
mod test_harness;

pub use bench::{BenchConfig, BenchHarness, BenchMode, BenchResult};
pub use config::{load_config_file, load_env_config};
pub use engine::{Engine, EngineBuilder, Ready, WantsConfig};
pub use error::{NuzoError, NuzoErrorKind, NuzoResult};
pub use nuzo_bytecode::Chunk;
pub use nuzo_config::Config;
pub use nuzo_helpers::builtins::{BuiltinFn, BuiltinRegistry};
pub use nuzo_ir::module_resolver::{MemoryResolver, ModuleResolver, ResolveError};
pub use nuzo_values::{HeapObject, Value, ValueTag};
pub use output::{Output, OutputSink};
// 注意：旧 `OutputSink` enum 仍由 `output` 模块导出（被 engine/session/bench/test_harness/main 使用）。
// 新 `OutputSink` trait 位于 `output_sink` 模块，通过 `nuzo_run::output_sink::OutputSink` 路径访问，
// 避免与旧 enum 同名冲突。仅顶级导出 `StdoutSink` / `StringSink` 两个实现类型。
pub use output_sink::{StdoutSink, StringSink};
pub use plugin::NuzoPlugin;
pub use session::Session;
pub use test_harness::{
    TestHarness, TestOutcome, TestResult, TestSummary, print_summary as print_test_summary,
};
