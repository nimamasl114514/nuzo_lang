//! # Nuzo ABI — 跨 crate 共享抽象层
//!
//! 集中封装 workspace 各 crate 共享的 trait、工具类型和 prelude，
//! 消除代码重复并建立统一的接口契约。
//!
//! ## 模块
//! - [`traits`] — 共享 trait（NuzoTrace, Tracer）
//! - [`index`] — 安全索引转换（SafeIndex）
//! - [`error_ext`] — 错误构造扩展（NuzoErrorExt）
//! - [`source_ext`] — 源码位置扩展（SourceLocationExt）
//! - [`prelude`] — 统一 prelude 导出

pub mod error_ext;
pub mod index;
pub mod prelude;
pub mod source_ext;
pub mod traits;

// 根级重导出，允许 `use nuzo_abi::NuzoErrorExt` 而非 `use nuzo_abi::error_ext::NuzoErrorExt`
pub use error_ext::NuzoErrorExt;
pub use source_ext::SourceLocationExt;
