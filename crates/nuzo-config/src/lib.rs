//! # nuzo_config — Nuzo 统一配置管理
//!
//! **层级**: L0（虚拟机层 / 配置层）—— 为 VM、GC、编译器与 Arena 提供分层配置管理，支持 TOML 文件、环境变量与代码默认值合并。
//!
//! **主要入口**: [`Config`], [`ConfigBuilder`], [`GcConfig`], [`VmConfig`], [`CompilerConfig`], [`ConfigError`]
//!
//! 零外部依赖的分层配置系统，支持 TOML 文件 + 环境变量 + 代码默认值三层合并。
//!
//! ## 设计原则
//!
//! - **零依赖**：自实现轻量 TOML 解析器，无 serde/toml 依赖
//! - **分层合并**：代码默认值 ← TOML 文件 ← 环境变量，后者覆盖前者
//! - **类型安全**：所有配置项有明确类型，解析失败回退默认值
//! - **向前兼容**：未知字段忽略+警告，缺失字段使用默认值
//! - **Builder 模式**：`Config::builder().with_toml_file(path).with_env().build()`
//!
//! ## 配置层级
//!
//! ```text
//! L1 代码默认值 (hardcoded defaults)
//!  ↓ 覆盖
//! L2 TOML 配置文件 (nuzo.toml / .nuzorc)
//!  ↓ 覆盖
//! L3 环境变量 (NUZO_*)
//! ```
//!
//! ## 使用示例
//!
//! ```rust,ignore
//! use nuzo_config::{Config, GcConfig, VmConfig};
//!
//! // 构建配置：默认值 + TOML 文件 + 环境变量
//! let config = Config::builder()
//!     .with_toml_file("nuzo.toml")   // 可选，文件不存在则跳过
//!     .with_env()                     // 读取 NUZO_* 环境变量
//!     .build();
//!
//! // 访问配置
//! println!("GC threshold: {}", config.gc.threshold);
//! println!("Max stack: {}", config.vm.max_stack_size);
//! ```

// Crate 元数据——外层属性形式（`#![inner_attr]` 在 stable Rust 不稳定）
// 层级 L2：nuzo_config 依赖 nuzo_proc（L1）与 nuzo_proc_core（L0 proc-macro 核心），
// 根据层级标注规则，至少为 L2（不能低于其最高依赖层级）。
#[nuzo_proc::crate_meta(layer = 2, description = "配置文件解析", entry_type = "Config")]
const _NUZO_CRATE_META_ANCHOR: () = ();

pub mod config;
pub mod error;
pub mod source;
pub mod toml_parser;
pub mod value;

pub use config::{
    ArenaConfig, CompilerConfig, Config, ConfigBuilder, FramePagingConfig, GcConfig, VmConfig,
};
pub use error::{ConfigError, ConfigResult};
