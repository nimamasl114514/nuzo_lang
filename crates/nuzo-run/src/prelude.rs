//! Prelude — one-line import for commonly used types.
//!
//! Shared ABI types (from `nuzo_abi::prelude`) are re-exported here, so
//! `use nuzo_run::prelude::*;` automatically brings in `Value`, `NuzoError`,
//! `SourceLocation`, etc. without a separate import.

// Shared ABI types (from nuzo-abi)
pub use nuzo_abi::prelude::*;

// Crate-specific types
pub use crate::{
    BenchHarness, BenchResult, Config, Engine, EngineBuilder, NuzoPlugin, NuzoResult, Output,
    OutputSink, Session,
};
pub use crate::{BuiltinFn, BuiltinRegistry, Chunk, HeapObject, ValueTag};
