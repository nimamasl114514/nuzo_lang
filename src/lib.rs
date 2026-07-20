//! # Nuzo Lang — Nuzo 脚本语言工作区根门面
//!
//! **层级**: L7（应用层 / 工作区门面）—— 薄转发层，统一暴露 `nuzo_run` 的公共 API，让下游用户通过单一 crate 使用引擎、值类型与配置。
//!
//! **主要入口**: [`Engine`], [`EngineBuilder`], [`Session`], [`Value`], [`Config`], [`Chunk`], [`BuiltinRegistry`]
//!
//! 此 crate 是 workspace 的便捷入口，转发 [`nuzo_run`] 门面的公共 API。
//! `nuzo_run` 是唯一的统一入口（run/debug/repl/auto/bench/test 模式合一）。
//!
//! ```ignore
//! use nuzo::{Engine, Value};
//! let engine = Engine::builder().with_default_config().build()?;
//! let out = engine.eval("1 + 2")?;
//! ```

#![allow(clippy::result_large_err)]

// Crate 元数据——外层属性形式（`#![inner_attr]` 在 stable Rust 不稳定）
#[nuzo_proc::crate_meta(layer = 7, description = "薄 re-export facade", entry_type = "Facade")]
const _NUZO_CRATE_META_ANCHOR: () = ();

pub use nuzo_run::{
    BuiltinFn, BuiltinRegistry, Chunk, Config, Engine, EngineBuilder, HeapObject, NuzoError,
    NuzoErrorKind, NuzoPlugin, NuzoResult, Output, OutputSink, Ready, Session, Value, ValueTag,
    WantsConfig, prelude,
};

/// 测试工具套件：性能基线管理、回归检测与统计量计算。
///
/// 详见 [`testkit::baseline`] 与 [`testkit::perf_regression`] 模块文档。
pub mod testkit;

use std::path::Path;

/// 快捷求值：创建临时引擎并执行一段 Nuzo 代码，返回结果值。
///
/// 适用于一次性脚本执行，不需要复用 Engine 的场景。
///
/// # Example
/// ```ignore
/// let result = nuzo::eval("1 + 2 * 3")?;
/// assert_eq!(result.to_string(), "7");
/// ```
pub fn eval(code: &str) -> NuzoResult<Value> {
    let engine = Engine::builder().with_default_config().build()?;
    engine.eval(code).map(|out| out.value)
}

/// 快捷运行文件：创建临时引擎并执行 .nuzo 脚本文件，返回结果值。
///
/// # Example
/// ```ignore
/// let result = nuzo::run_file("script.nuzo")?;
/// ```
pub fn run_file(path: impl AsRef<Path>) -> NuzoResult<Value> {
    let engine = Engine::builder().with_default_config().build()?;
    engine.run_file(path.as_ref()).map(|out| out.value)
}

/// 返回 Nuzo 语言版本号。
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
