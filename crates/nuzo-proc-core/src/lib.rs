//! # nuzo_proc_core — Nuzo 过程宏开发核心工具库
//!
//! **层级**: L0（基础基础设施层）—— 为工作区内所有过程宏提供统一展开逻辑与常量管理，支撑编译期代码生成。
//!
//! **主要入口**: [`hardcode`], [`attr`], [`diag`], [`match_sync::expand_match_sync`], [`trace_derive::expand_trace`], [`test_attr::expand_nuzo_test_attr`]
//!
//! 本 crate 为 Nuzo 工作区内所有过程宏提供统一的开发基础设施。
//! 采用**双 crate 架构**：本 crate 为普通库（可被任何 crate 引用），
//! `nuzo_proc` 为 proc-macro 入口（仅包含 `#[proc_macro_*]` 函数）。
//!
//! ## 模块概览
//!
//! | 模块 | 功能 | 依赖 |
//! |------|------|------|
//! | [`hardcode`] | 硬编码常量管理框架（零依赖，始终可用） | 无 |
//! | [`diag`] | 精确错误报告（span-aware 诊断） | `proc_macro` feature |
//! | [`token`] | TokenStream 操作工具（构建、解析、变换） | `proc_macro` feature |
//! | [`attr`] | 声明式属性解析框架 | `proc_macro` feature |
//! | [`validate`] | 编译期校验工具 | `proc_macro` feature |
//! | [`nuzo`] | Nuzo 生态集成（crate 路径发现、导入生成） | `proc_macro` feature |
//! | [`discover`] | `test_bind::discover!` 宏核心（模块清单发现） | `proc_macro` feature |
//! | [`parse_utils`] | 共享解析工具（字面量提取、camelCase 转换） | `proc_macro` feature |
//! | [`match_sync`] | `#[derive(MatchSync)]` 核心展开逻辑 | `proc_macro` feature |
//! | [`trace_derive`] | `#[derive(Trace)]` GC 自动 trace 展开 | `proc_macro` feature |
//! | [`test_attr`] | `#[nuzo_test]` 属性宏核心展开 | `proc_macro` feature |
//! | [`opcode_sync_derive`] | `#[derive(OpcodeSync)]` SSOT 自动同步 | `proc_macro` feature |
//!
//! ## Feature Flags
//!
//! | Feature | 默认 | 说明 |
//! |---------|------|------|
//! | `proc_macro` | 是 | 启用所有过程宏模块（引入 syn/proc-macro2/quote/rustpython-parser） |
//! | `env-override` | 是 | hardcode 常量的环境变量覆盖 |
//! | `json-export` | 是 | hardcode 常量的 JSON 导出 |
//! | `test-utils` | 否 | 暴露测试辅助函数（clear() 等） |
//!
//! **自动注册**：`define_constants!` 宏使用 `ctor` crate 在程序启动时自动注册常量，
//! 无需手动调用 `__register_constants()`。`ctor` 是轻量级依赖（无重依赖），始终启用。
//!
//! 仅需 hardcode 常量管理的轻量依赖方（如 nuzo_core）应使用
//! `default-features = false` 避开 syn 重依赖。

// 重导出 ctor，供 define_constants! 宏在调用方 crate 中使用
// 这样调用方无需显式依赖 ctor crate
pub use ctor;

// hardcode 常量管理模块（零依赖，始终可用，不需要 proc_macro feature）
pub mod hardcode;

// 文档同步模块（零依赖，始终可用，供 define_opcodes! 宏生成的 OPCODE_DOCS 常量使用）
pub mod doc_sync;

// 过程宏相关模块（依赖 syn/proc-macro2/quote/rustpython-parser）
// 仅在启用 proc_macro feature 时编译
#[cfg(feature = "proc_macro")]
pub mod attr;
#[cfg(feature = "proc_macro")]
pub mod crate_meta;
#[cfg(feature = "proc_macro")]
pub mod diag;
#[cfg(feature = "proc_macro")]
pub mod discover;
#[cfg(feature = "proc_macro")]
pub mod error_kind;
#[cfg(feature = "proc_macro")]
pub mod expr_visitor_derive;
#[cfg(feature = "proc_macro")]
pub mod match_sync;
#[cfg(feature = "proc_macro")]
pub mod nuzo;
#[cfg(feature = "proc_macro")]
pub mod opcode_sync_derive;
#[cfg(feature = "proc_macro")]
pub mod parse_utils;
#[cfg(feature = "proc_macro")]
pub mod path;
#[cfg(feature = "proc_macro")]
pub mod test_attr;
#[cfg(feature = "proc_macro")]
pub mod token;
#[cfg(feature = "proc_macro")]
pub mod trace_derive;
#[cfg(feature = "proc_macro")]
pub mod validate;
