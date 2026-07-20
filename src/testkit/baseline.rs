//! # 性能基線管理
//!
//! 提供基線數據的結構定義、環境信息收集與磁碟持久化。
//!
//! ## 數據結構
//!
//! - [`EnvironmentInfo`] — 運行環境信息（OS、CPU、版本等）
//! - [`BenchmarkMetric`] — 單項基準測試的統計指標
//! - [`BaselineData`] — 完整基線快照（含環境與全部指標）
//! - [`BaselineManager`] — 基線文件的加載與保存
//!
//! ## 存儲格式
//!
//! 基線以 JSON 格式存儲，默認路徑為 `benchmarks/baseline/latest.json`。
//! 每次更新基線時可額外備份以 commit hash 命名的副本，供歷史趨勢分析。

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

// ============================================================================
// 錯誤類型
// ============================================================================

/// 基線操作錯誤
///
/// 涵蓋 IO 與 JSON 序列化/反序列化兩類錯誤。
#[derive(Debug)]
pub enum BaselineError {
    /// 文件讀寫錯誤
    Io(std::io::Error),
    /// JSON 解析或生成錯誤
    Json(serde_json::Error),
}

impl fmt::Display for BaselineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "IO 錯誤: {e}"),
            Self::Json(e) => write!(f, "JSON 錯誤: {e}"),
        }
    }
}

impl std::error::Error for BaselineError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            Self::Json(e) => Some(e),
        }
    }
}

impl From<std::io::Error> for BaselineError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<serde_json::Error> for BaselineError {
    fn from(e: serde_json::Error) -> Self {
        Self::Json(e)
    }
}

// ============================================================================
// 環境信息
// ============================================================================

/// 運行環境信息
///
/// 記錄基線採集時的運行環境，用於跨環境對比時的上下文參考。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvironmentInfo {
    /// 操作系統（如 "linux"、"windows"、"macos"）
    pub os: String,

    /// CPU 架構（如 "x86_64"、"aarch64"）
    pub arch: String,

    /// 可用並行度（邏輯 CPU 數）
    pub cpu_count: usize,

    /// Nuzo 語言版本號
    pub nuzo_version: String,

    /// Rust 工具鏈版本（編譯期常量）
    pub rust_version: String,
}

/// 收集當前運行環境信息
///
/// 使用 `std::env::consts` 與 `std::thread::available_parallelism`
/// 採集環境信息，無需外部依賴。
pub fn collect_environment_info() -> EnvironmentInfo {
    let cpu_count = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);

    EnvironmentInfo {
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        cpu_count,
        nuzo_version: env!("CARGO_PKG_VERSION").to_string(),
        rust_version: env!("CARGO_PKG_RUST_VERSION").to_string(),
    }
}

// ============================================================================
// 基線數據結構
// ============================================================================

/// 單項基準測試的統計指標
///
/// 存儲於基線文件中，供後續對比使用。
/// 注意：基線僅存儲統計量，不存儲原始樣本數據。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchmarkMetric {
    /// 基準測試名稱（通常等於 ID）
    pub name: String,

    /// 測量單位（如 "ns"、"ops/s"）
    pub unit: String,

    /// 近似迭代次數
    pub iterations: u64,

    /// 樣本均值
    pub mean: f64,

    /// 樣本標準差
    pub std_dev: f64,

    /// 中位數
    pub median: f64,

    /// 第 95 百分位數
    pub p95: f64,

    /// 第 99 百分位數
    pub p99: f64,

    /// 樣本數量
    pub sample_size: usize,
}

/// 完整基線快照
///
/// 匯聚一次基線採集的全部信息：版本、commit、時間戳、環境與各項指標。
/// 可序列化為 JSON 存儲，亦可從 JSON 反序列化加載。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BaselineData {
    /// Nuzo 語言版本號
    pub version: String,

    /// Git commit hash（短格式）
    pub commit_hash: String,

    /// 採集時間戳（ISO 8601 格式，如 "1234567890Z"）
    pub timestamp: String,

    /// 採集時的運行環境
    pub environment: EnvironmentInfo,

    /// 各項基準測試的指標（鍵為基準測試 ID，如 "B001"）
    pub benchmarks: BTreeMap<String, BenchmarkMetric>,
}

// ============================================================================
// 基線管理器
// ============================================================================

/// 基線文件管理器
///
/// 封裝基線 JSON 文件的加載與保存邏輯。
///
/// # 默認路徑
///
/// - 加載/保存默認路徑：`{baseline_dir}/latest.json`
/// - 可通過 `path` 參數指定自定義路徑
///
/// # 示例
///
/// ```ignore
/// use nuzo::testkit::baseline::{BaselineManager, BaselineData};
///
/// let manager = BaselineManager::new();
/// let data: BaselineData = manager.load(None)?;
/// manager.save(&data, None)?;
/// ```
pub struct BaselineManager {
    /// 基線存儲目錄
    baseline_dir: PathBuf,
}

impl BaselineManager {
    /// 創建默認管理器
    ///
    /// 基線目錄設為 `benchmarks/baseline`。
    pub fn new() -> Self {
        Self { baseline_dir: PathBuf::from("benchmarks/baseline") }
    }

    /// 創建管理器並指定基線目錄
    ///
    /// 通常用於測試或自定義存儲位置。
    pub fn with_dir<P: AsRef<Path>>(dir: P) -> Self {
        Self { baseline_dir: PathBuf::from(dir.as_ref()) }
    }

    /// 加載基線數據
    ///
    /// - `path = None` — 加載 `{baseline_dir}/latest.json`
    /// - `path = Some(p)` — 加載指定路徑
    ///
    /// # 錯誤
    ///
    /// 文件不存在或 JSON 解析失敗時返回 [`BaselineError`]。
    pub fn load(&self, path: Option<&str>) -> Result<BaselineData, BaselineError> {
        let path = self.resolve_path(path);
        let file = std::fs::File::open(&path)?;
        let reader = std::io::BufReader::new(file);
        let data = serde_json::from_reader(reader)?;
        Ok(data)
    }

    /// 保存基線數據
    ///
    /// - `path = None` — 保存至 `{baseline_dir}/latest.json`
    /// - `path = Some(p)` — 保存至指定路徑
    ///
    /// 保存前會自動創建父目錄。輸出格式為美化後的 JSON。
    ///
    /// # 錯誤
    ///
    /// 目錄創建失敗或寫入失敗時返回 [`BaselineError`]。
    pub fn save(&self, data: &BaselineData, path: Option<&str>) -> Result<(), BaselineError> {
        let path = self.resolve_path(path);

        // 確保父目錄存在
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)?;
        }

        let json = serde_json::to_string_pretty(data)?;
        std::fs::write(&path, json)?;
        Ok(())
    }

    /// 解析路徑
    ///
    /// `Some(p)` 返回 `PathBuf::from(p)`，`None` 返回默認的 `latest.json`。
    fn resolve_path(&self, path: Option<&str>) -> PathBuf {
        match path {
            Some(p) => PathBuf::from(p),
            None => self.baseline_dir.join("latest.json"),
        }
    }
}

impl Default for BaselineManager {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// 單元測試
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_environment_info_collection() {
        let env = collect_environment_info();
        assert!(!env.os.is_empty(), "OS 不應為空");
        assert!(!env.arch.is_empty(), "ARCH 不應為空");
        assert!(env.cpu_count >= 1, "CPU 數量至少為 1");
        assert!(!env.nuzo_version.is_empty(), "版本號不應為空");
    }

    #[test]
    fn test_baseline_save_and_load_roundtrip() {
        // 使用臨時目錄避免污染工作區
        let tmp_dir = std::env::temp_dir().join(format!(
            "nuzo_testkit_baseline_{}",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));

        let manager = BaselineManager::with_dir(&tmp_dir);

        let original = BaselineData {
            version: "0.5.0".to_string(),
            commit_hash: "abc1234".to_string(),
            timestamp: "1234567890Z".to_string(),
            environment: EnvironmentInfo {
                os: "linux".to_string(),
                arch: "x86_64".to_string(),
                cpu_count: 8,
                nuzo_version: "0.5.0".to_string(),
                rust_version: "1.88.0".to_string(),
            },
            benchmarks: {
                let mut m = BTreeMap::new();
                m.insert(
                    "B001".to_string(),
                    BenchmarkMetric {
                        name: "B001".to_string(),
                        unit: "ns".to_string(),
                        iterations: 10000,
                        mean: 1234.5,
                        std_dev: 56.7,
                        median: 1230.0,
                        p95: 1300.0,
                        p99: 1350.0,
                        sample_size: 10,
                    },
                );
                m
            },
        };

        // 保存
        manager.save(&original, None).expect("保存應成功");

        // 加載並對比
        let loaded = manager.load(None).expect("加載應成功");
        assert_eq!(loaded.version, original.version);
        assert_eq!(loaded.commit_hash, original.commit_hash);
        assert_eq!(loaded.timestamp, original.timestamp);
        assert_eq!(loaded.environment.os, original.environment.os);
        assert_eq!(loaded.benchmarks.len(), 1);
        assert_eq!(loaded.benchmarks["B001"].mean, 1234.5);

        // 清理臨時目錄
        let _ = std::fs::remove_dir_all(&tmp_dir);
    }

    #[test]
    fn test_baseline_load_missing_file() {
        let manager = BaselineManager::with_dir("/nonexistent/path/that/should/not/exist");
        let result = manager.load(None);
        assert!(result.is_err(), "加載不存在的文件應失敗");
    }
}
