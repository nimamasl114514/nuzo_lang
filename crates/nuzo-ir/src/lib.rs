//! # Nuzo IR — Nuzo 目标无关的中间表示
//!
//! **层级**: L4（语言核心层 / 中间表示层）—— 位于 AST 与字节码后端之间，提供目标无关的 SSA 风格 IR、控制流图与优化接口。
//!
//! **主要入口**: [`ValueRef`], [`BasicBlockId`], [`IrFunctionId`], [`IrOp`], [`IrModule`], [`builder::IrBuilder`], [`optimize`]
//!
//! **Crate 定位**: Nuzo 编译器的 IR 层，提供目标无关的三地址码中间表示
//!
//! ## 架构概览
//! 本 crate 是编译器前端（AST）与后端（字节码）之间的**中间层**，
//! 负责定义 SSA 风格的 IR 数据结构、控制流图（CFG）、以及 IR 级别的优化接口。
//!
//! ## 核心职责
//!
//! ### 1. IR 数据类型 (`types` 模块)
//! - **ID 类型**: `ValueRef`, `BasicBlockId`, `IrFunctionId` (Newtype 包装器)
//! - **常量与运算符**: `IrConstant`, `IrBinOp`, `IrUnaryOp`
//! - **IR 操作码**: `IrOp` 枚举（三地址指令集）
//! - **控制流结构**: `BasicBlock`, `IrFunction`, `IrModule`
//!
//! ### 2. 设计原则
//! - **目标无关**: 不依赖 `nuzo_core::Value` 或具体寄存器分配
//! - **SSA 风格**: 使用虚拟寄存器引用 (`ValueRef`)，每个值只赋值一次
//! - **三地址码**: 每条指令最多 1 个目标 + 2 个源操作数
//! - **内存高效**: 使用 `Arc<str>` 共享字符串，避免克隆开销
//!
//! ### 3. 运算符映射 SSOT（Single Source of Truth）
//! `types` 模块中的 `From<ast::BinaryOp> for IrBinOp` 和 `From<ast::UnaryOp> for IrUnaryOp`
//! 是 AST 运算符到 IR 运算符的**唯一映射定义**。所有需要转换的代码（如 `builder.rs::build_binary`）
//! 都应使用 `.into()` 或 `IrBinOp::from(op)`。这消除了编译器层和 IR 层的重复 match。
//! 如果 `BinaryOp`/`UnaryOp` 新增变体，`From` 实现中的 match 将非 exhaustive，编译器直接报错。
//!
//! ## 依赖关系
//! - **nuzo_core**: XxHashMap, SourceLocation 等核心基础类型
//! - **nuzo_frontend**: AST 类型（用于 AST → IR 转换）
//!
//! ## 层级位置
//! ```ignore
//! L0 (nuzo_proc_core, 含 hardcode 模块) → L1 (nuzo_core, nuzo_proc, nuzo_signal)
//! → L2 (nuzo_values, nuzo_opcode) → L3 (nuzo_bytecode, nuzo_frontend)
//! → L4 (**nuzo_ir**, nuzo_compiler, nuzo_helpers, nuzo_error) → L5 (nuzo_vm) → ...
//! ```

#![allow(clippy::result_large_err)]

// Crate 元数据——外层属性形式（`#![inner_attr]` 在 stable Rust 不稳定）
#[nuzo_proc::crate_meta(layer = 4, description = "中间表示与优化器", entry_type = "IrBuilder")]
const _NUZO_CRATE_META_ANCHOR: () = ();

// 核心数据类型定义
pub mod types;

// 模块路径解析器 trait（由 nuzo_run::Engine 实现，避免反向依赖）
pub mod module_resolver;

// 错误类型
pub mod error;

// Re-export key types for convenience
pub use error::{IrBuildError, IrValidationError, ValidationWarning};
// Re-export error code system (错误代码体系)
pub use error::{IrErrorCategory, IrErrorCode, IrErrorSeverity};
// Re-export display types for validator access
pub use display::ValidationResult;

// IR 构建器（AST → IR 转换）
pub mod builder;

// 显示格式化与验证
pub mod display;

// IR 优化 Pass（常量折叠、恒等消除、死代码消除）
pub mod optimize;

// ── types 模块导出 ──────────────────────────────────────────────
// ID 类型 (Newtype 包装器)
pub use types::{BasicBlockId, IrFunctionId, ValueRef};
// 常量与运算符
pub use types::{IrBinOp, IrConstant, IrUnaryOp};
// IR 操作码
pub use types::IrOp;
// 控制流结构
pub use types::{BasicBlock, CaptureDesc, CaptureSource, IrFunction, IrModule};
