//! # Nuzo Compiler — IR 路径编译器
//!
//! **层级**: L5（编译器层）—— 将 AST 经由 IR 编译为寄存机字节码。
//!
//! **主要入口**: [`Compiler`], [`CompilerBuilder`], [`CompileError`], [`CodeGenerator`], [`RegisterAllocator`], [`LsraAllocator`]
//!
//! ## 编译流程
//!
//! ```text
//! Source → Lexer → Parser → AST → IrBuilder → IR → CodeGenerator → Chunk
//! ```
//!
//! 所有编译均通过 IR 路径完成。旧的 AST 直编译路径已在 0.3.0 版本中移除。
//!
//! ## 模块职责
//!
//! | 模块 | 职责 | 入口类型 |
//! |------|------|----------|
//! | [`compiler`] | 编译器主入口、编译流程 | [`Compiler`](compiler::Compiler) |
//! | [`codegen`] | IR → 字节码代码生成 | [`CodeGenerator`](codegen::CodeGenerator) |
//! | [`allocator`] | 基于信号槽的寄存器分配器 | [`RegisterAllocator`](allocator::RegisterAllocator) |

#![allow(clippy::result_large_err)]

// Crate 元数据——外层属性形式（`#![inner_attr]` 在 stable Rust 不稳定）
#[nuzo_proc::crate_meta(
    layer = 5,
    description = "编译器核心（AST→IR→字节码）",
    entry_type = "Compiler"
)]
const _NUZO_CRATE_META_ANCHOR: () = ();

pub mod allocator;
pub mod codegen;
pub mod compiler;
pub mod prelude;
pub mod reg_manager;
pub mod reg_pool;
pub mod usage_counter;
pub mod value_tracker;

// --- compiler: 公开 API ---
pub use compiler::{
    COMPILE_FINISHED_KEY, COMPILE_FUNCTION_DONE_KEY, COMPILE_STARTED_KEY, CompileError, Compiler,
    CompilerBuilder, compiler_bus,
};

// --- codegen: 公开 API (IR → Bytecode) ---
pub use codegen::{CodeGenerator, CodegenError};

// --- allocator: 公开 API（含 LSRA） ---
pub use allocator::{
    AllocError, Interval, LsraAllocator, NudConfig, RegisterAllocator, SlotHandle, build_intervals,
};
// SlotOwner 定义在 nuzo_signal 中，通过 re-export 暴露
pub use nuzo_signal::SlotOwner;

#[cfg(test)]
mod peephole_tests;
