//! nuzo-abi 统一 prelude
//!
//! 集中导出跨 crate 共享的类型，减少各 crate prelude 的重复。

pub use crate::error_ext::NuzoErrorExt;
pub use crate::index::{IndexOverflowError, SafeIndex, SafeU16, SafeU32, SafeU8};
pub use crate::source_ext::SourceLocationExt;
pub use crate::traits::{NuzoTrace, Tracer};

// 从 nuzo-core 重导出共享类型（这些在各 crate prelude 中重复出现）
pub use nuzo_core::SourceLocation;
pub use nuzo_core::Value;
pub use nuzo_core::{NuzoError, NuzoErrorKind};
