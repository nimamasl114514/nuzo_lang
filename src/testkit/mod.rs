//! # Nuzo 性能测试工具包
//!
//! 提供性能基準測試、基線管理與回歸檢測的基礎設施。
//!
//! ## 模組結構
//!
//! - [`baseline`] — 基線數據結構與持久化管理
//! - [`perf_regression`] — 性能基準測試執行與統計分析
//!
//! ## 設計原則
//!
//! - 純 std + serde 實現，不引入額外外部依賴
//! - 統計量（mean/stddev/median/p95/p99）由本模組自行計算
//! - 基線文件格式為 JSON，便於 CI/CD 解析與版本控制

pub mod baseline;
pub mod perf_regression;
