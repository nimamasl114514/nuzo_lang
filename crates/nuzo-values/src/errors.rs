//! Error types re-exported from `nuzo_core::error`.
//!
//! From v0.5.0, `InternalError`, `NuzoErrorKind`, `NuzoError`, `VmDiagnosis`
//! are defined in `nuzo_core::error` and re-exported here for backward compatibility.

pub use nuzo_core::SourceLocation;
pub use nuzo_core::error::{InternalError, NuzoError, NuzoErrorKind, VmDiagnosis};
