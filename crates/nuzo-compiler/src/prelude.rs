//! # Nuzo Compiler Prelude
//!
//! Re-exports the most commonly used types from the `nuzo_compiler` crate.
//! Intended to be glob-imported: `use nuzo_compiler::prelude::*;`
//!
//! This follows Rust ecosystem conventions (e.g., `std::prelude`, `bevy::prelude`).

// --- Compiler core ---
pub use crate::compiler::{CompileError, Compiler, CompilerBuilder, compiler_bus};

// --- Compiler config (from nuzo_config) ---
pub use nuzo_config::CompilerConfig;

// --- Codegen (IR → Bytecode) ---
pub use crate::codegen::{CodeGenerator, CodegenError};

// --- Allocator ---
pub use crate::allocator::{AllocError, RegisterAllocator, SlotHandle};

// --- LSRA ---
pub use crate::allocator::{Interval, LsraAllocator, NudConfig, build_intervals};

// --- SlotOwner (from nuzo_signal, re-exported in crate root) ---
pub use crate::SlotOwner;
