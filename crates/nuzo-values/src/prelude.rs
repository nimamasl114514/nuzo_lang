//! Nuzo Values prelude — a convenient glob import of the most commonly used types.
//!
//! This module follows the Rust ecosystem convention (e.g., `std::prelude`, `tokio::prelude`)
//! of providing a single `use nuzo_values::prelude::*;` that brings all essential types into scope.
//!
//! # Usage
//!
//! ```rust
//! use nuzo_values::prelude::*;
//! ```
//!
//! # What's included
//!
//! - **Shared ABI types** (via `nuzo_abi::prelude`): `Value`, `NuzoError`, `NuzoErrorKind`,
//!   `SourceLocation`, `Tracer`, `NuzoTrace`, `SafeIndex`, `SafeU8`, `SafeU16`, `SafeU32`,
//!   `IndexOverflowError`, `NuzoErrorExt`, `SourceLocationExt`
//! - **Core value types**: `ValueExt`, `ValueTag`
//! - **Heap objects**: `HeapObject`, `NuzoDict`, `SmallDict`, `LargeDict`
//! - **Errors** (crate-specific): `InternalError`, `VmDiagnosis`
//! - **Functions**: `FunctionPrototype`, `DebugInfo`, `PrototypeDebugInfo`
//! - **Runtime context**: `RuntimeContext`
//! - **Closure captures**: `CaptureMode`, `CaptureInfo`, `CapturedVar`
//! - **Constants**: `NIL`, `TRUE`, `FALSE`
//! - **Traits**: `NuzoType`
//! - **Misc**: `RangeValue`, `RangeEnd`, `DeadCodeRecord`, `DeadCodeReason`, `FoldRecord`, `InlineRecord`

// ---------------------------------------------------------------------------
// Shared ABI types (from nuzo-abi)
// ---------------------------------------------------------------------------
pub use nuzo_abi::prelude::*;

// ---------------------------------------------------------------------------
// Core value types (crate-specific, not in nuzo-abi)
// ---------------------------------------------------------------------------
pub use crate::value::ValueExt;
pub use crate::value::ValueTag;

// ---------------------------------------------------------------------------
// Heap objects
// ---------------------------------------------------------------------------
pub use crate::heap::HeapObject;
pub use crate::nuzo_dict::LargeDict;
pub use crate::nuzo_dict::NuzoDict;
pub use crate::nuzo_dict::SmallDict;

// ---------------------------------------------------------------------------
// Errors (crate-specific, not in nuzo-abi)
// ---------------------------------------------------------------------------
pub use crate::errors::InternalError;
pub use crate::errors::VmDiagnosis;

// ---------------------------------------------------------------------------
// Functions / prototypes
// ---------------------------------------------------------------------------
pub use crate::function::DeadCodeReason;
pub use crate::function::DeadCodeRecord;
pub use crate::function::DebugInfo;
pub use crate::function::FoldRecord;
pub use crate::function::FunctionPrototype;
pub use crate::function::InlineRecord;
pub use crate::function::PrototypeDebugInfo;

// ---------------------------------------------------------------------------
// Runtime context
// ---------------------------------------------------------------------------
pub use crate::context::RuntimeContext;

// ---------------------------------------------------------------------------
// Closure captures
// ---------------------------------------------------------------------------
pub use crate::heap::CaptureInfo;
pub use crate::heap::CaptureMode;
pub use crate::heap::CapturedVar;

// ---------------------------------------------------------------------------
// Constants (not in nuzo-abi)
// ---------------------------------------------------------------------------
pub use crate::value::FALSE;
pub use crate::value::NIL;
pub use crate::value::TRUE;

// ---------------------------------------------------------------------------
// Traits
// ---------------------------------------------------------------------------
pub use crate::traits::NuzoType;

// ---------------------------------------------------------------------------
// Misc
// ---------------------------------------------------------------------------
pub use crate::heap::RangeEnd;
pub use crate::value::RangeValue;
