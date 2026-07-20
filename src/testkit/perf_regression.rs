//! # 性能基準測試與回歸檢測
//!
//! 提供基準測試的執行框架、統計分析與結果數據結構。
//!
//! ## 核心類型
//!
//! - [`BenchmarkConfig`] — 基準測試配置（採樣次數、預熱輪數）
//! - [`Statistics`] — 統計量集合（mean/stddev/median/p95/p99）
//! - [`ComparisonTarget`] — 比較方向（越小越好 / 越大越好）
//! - [`BenchmarkResult`] — 單項基準測試結果
//!
//! ## 執行入口
//!
//! [`run_all_benchmarks_with_options`] 運行全部內置基準測試，
//! 支持進度回調與 E2E 跳過。

use std::cmp::Ordering;

// ============================================================================
// 配置與枚舉
// ============================================================================

/// 基準測試配置
///
/// 控制採樣次數與預熱行為。所有字段均有合理默認值。
#[derive(Debug, Clone)]
pub struct BenchmarkConfig {
    /// 每項基準測試的採樣次數（計入統計）
    pub sample_count: usize,

    /// 預熱輪數（不計入統計，用於穩定 CPU 緩存等）
    pub warmup_rounds: usize,
}

impl Default for BenchmarkConfig {
    fn default() -> Self {
        // 采样 30 + 预热 5：减少系统噪声对短时测试的影响
        // （原 10+2 在 B004 等含内存分配的测试上波动可达 ±70%）
        Self { sample_count: 30, warmup_rounds: 5 }
    }
}

/// 比較方向
///
/// 定義基準測試指標的優化方向，用於回歸判定。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComparisonTarget {
    /// 越小越好（如耗時、內存佔用）
    LowerIsBetter,
    /// 越大越好（如吞吐量、ops/s）
    HigherIsBetter,
}

/// 基準測試條目類型別名
///
/// 元組依次為：測試 ID、可讀名稱、比較方向、測量函數指針。
type BenchmarkEntry = (&'static str, &'static str, ComparisonTarget, fn() -> f64);

// ============================================================================
// 統計量
// ============================================================================

/// 統計量集合
///
/// 由原始樣本計算得出，存儲均值、標準差、中位數及百分位數。
#[derive(Debug, Clone)]
pub struct Statistics {
    /// 原始樣本數據
    pub samples: Vec<f64>,

    /// 樣本均值
    pub mean: f64,

    /// 樣本標準差（總體標準差，非樣本標準差）
    pub stddev: f64,

    /// 中位數
    pub median: f64,

    /// 第 95 百分位數
    pub p95: f64,

    /// 第 99 百分位數
    pub p99: f64,
}

/// 由樣本計算統計量
///
/// 採用線性插值法計算百分位數。
/// 樣本為空時所有統計量返回 0.0。
fn compute_statistics(samples: Vec<f64>) -> Statistics {
    let n = samples.len();
    if n == 0 {
        return Statistics { samples, mean: 0.0, stddev: 0.0, median: 0.0, p95: 0.0, p99: 0.0 };
    }

    // 均值
    let mean = samples.iter().sum::<f64>() / n as f64;

    // 標準差（總體標準差）
    let variance: f64 = samples.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / n as f64;
    let stddev = variance.sqrt();

    // 排序後計算中位數與百分位數
    let mut sorted = samples.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(Ordering::Equal));

    let median =
        if n.is_multiple_of(2) { (sorted[n / 2 - 1] + sorted[n / 2]) / 2.0 } else { sorted[n / 2] };

    // 百分位數：線性插值法
    let percentile = |p: f64| -> f64 {
        if n == 1 {
            return sorted[0];
        }
        let rank = (p / 100.0) * (n - 1) as f64;
        let lower = rank.floor() as usize;
        let upper = rank.ceil() as usize;
        let frac = rank - lower as f64;
        sorted[lower] * (1.0 - frac) + sorted[upper.min(n - 1)] * frac
    };

    let p95 = percentile(95.0);
    let p99 = percentile(99.0);

    Statistics { samples, mean, stddev, median, p95, p99 }
}

// ============================================================================
// 基準測試結果
// ============================================================================

/// 單項基準測試結果
///
/// 匯聚一項基準測試的 ID、名稱、單位、統計量與比較方向。
#[derive(Debug, Clone)]
pub struct BenchmarkResult {
    /// 基準測試 ID（如 "B001"）
    pub id: String,

    /// 人類可讀名稱
    pub name: String,

    /// 測量單位（如 "ns"、"ops/s"）
    pub unit: String,

    /// 統計量
    pub statistics: Statistics,

    /// 比較方向
    pub comparison: ComparisonTarget,
}

impl BenchmarkResult {
    /// 判定是否達標
    ///
    /// 簡化判定：樣本數 >= 1 且均值為有限值即視為達標。
    /// 真實場景應結合基線閾值判定，此處僅供 run 子命令顯示狀態。
    pub fn meets_target(&self) -> bool {
        !self.statistics.samples.is_empty() && self.statistics.mean.is_finite()
    }
}

// ============================================================================
// 基準測試執行
// ============================================================================

/// 進度回調類型
///
/// 參數依次為：當前完成序號（從 1 開始）、總項數、當前測試名稱。
/// 生命週期 `'a` 綁定到調用方作用域，允許閉包捕獲局部變量。
pub type ProgressCallback<'a> = dyn Fn(usize, usize, &str) + 'a;

/// 運行全部基準測試
///
/// 依序執行內置基準測試，收集樣本並計算統計量。
///
/// # 參數
///
/// - `config` — 基準測試配置（採樣次數、預熱輪數）
/// - `skip_e2e` — 是否跳過 E2E 端到端測試（B004）
/// - `progress` — 可選的進度回調
///
/// # 返回
///
/// 全部基準測試的結果向量。
pub fn run_all_benchmarks_with_options<'a>(
    config: &BenchmarkConfig,
    skip_e2e: bool,
    progress: Option<&'a ProgressCallback<'a>>,
) -> Vec<BenchmarkResult> {
    // 內置基準測試清單：(id, name, comparison, 函數指針)
    let mut benchmarks: Vec<BenchmarkEntry> = vec![
        ("B001", "算術循環（10 萬次加法）", ComparisonTarget::LowerIsBetter, bench_arithmetic),
        ("B002", "字符串拼接（1 萬次）", ComparisonTarget::LowerIsBetter, bench_string_concat),
        ("B003", "Vec 分配（1 萬次 push）", ComparisonTarget::LowerIsBetter, bench_vec_alloc),
    ];

    if !skip_e2e {
        benchmarks.push((
            "B004",
            "E2E 模擬（綜合工作負載）",
            ComparisonTarget::LowerIsBetter,
            bench_e2e_mock,
        ));
    }

    let total = benchmarks.len();
    let mut results = Vec::with_capacity(total);

    for (idx, (id, name, comparison, func)) in benchmarks.iter().enumerate() {
        // 進度回調
        if let Some(cb) = progress {
            cb(idx + 1, total, name);
        }

        // 預熱（不計入採樣）
        for _ in 0..config.warmup_rounds {
            std::hint::black_box(func());
        }

        // 採樣
        let samples: Vec<f64> = (0..config.sample_count).map(|_| func()).collect();
        let statistics = compute_statistics(samples);

        results.push(BenchmarkResult {
            id: (*id).to_string(),
            name: (*name).to_string(),
            unit: "ns".to_string(),
            statistics,
            comparison: *comparison,
        });
    }

    results
}

// ============================================================================
// 內置基準測試
// ============================================================================

/// B001：算術循環
///
/// 執行 10 萬次整數加法，測量原始計算吞吐。
fn bench_arithmetic() -> f64 {
    const ITERATIONS: u64 = 100_000;
    let start = std::time::Instant::now();
    let mut sum: u64 = 0;
    for i in 0..ITERATIONS {
        // black_box 包裹每次累加结果，阻止 LLVM 把循环折叠成 closed-form
        sum = std::hint::black_box(sum.wrapping_add(i));
    }
    start.elapsed().as_nanos() as f64
}

/// B002：字符串拼接
///
/// 執行 1 萬次字符串 push_str，測量字符串操作性能。
fn bench_string_concat() -> f64 {
    const ITERATIONS: usize = 10_000;
    use nuzo_run::Engine;
    let engine = Engine::quick().expect("Engine 创建失败");
    let script = format!("s = \"\"; for i in 0..{} {{ s = s + i + \",\" }}", ITERATIONS);
    let start = std::time::Instant::now();
    let result = engine.run(&script).expect("脚本执行失败");
    std::hint::black_box(result);
    start.elapsed().as_nanos() as f64
}

/// B003：Vec 分配
///
/// 執行 1 萬次 Vec::push，測量動態數組分配性能。
fn bench_vec_alloc() -> f64 {
    const ITERATIONS: usize = 10_000;
    let start = std::time::Instant::now();
    let mut v: Vec<u64> = Vec::with_capacity(ITERATIONS);
    for i in 0..ITERATIONS {
        v.push(i as u64);
    }
    std::hint::black_box(v.len());
    start.elapsed().as_nanos() as f64
}

/// B004：E2E 模擬
///
/// 模擬端到端工作負載（算術 + 字符串 + Vec 組合），耗時較長。
/// 可通過 `skip_e2e = true` 跳過。
fn bench_e2e_mock() -> f64 {
    const ITERATIONS: usize = 5_000;
    let start = std::time::Instant::now();
    let mut sum: u64 = 0;
    let mut s = String::new();
    let mut v: Vec<u64> = Vec::with_capacity(ITERATIONS);
    for i in 0..ITERATIONS {
        sum = sum.wrapping_add(i as u64);
        s.push_str(&i.to_string());
        v.push(i as u64);
    }
    std::hint::black_box((sum, s.len(), v.len()));
    start.elapsed().as_nanos() as f64
}

// ============================================================================
// 單元測試
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_benchmark_config_default() {
        let config = BenchmarkConfig::default();
        assert!(config.sample_count > 0, "採樣次數應 > 0");
        assert!(config.warmup_rounds > 0, "預熱輪數應 > 0");
    }

    #[test]
    fn test_compute_statistics_empty() {
        let stats = compute_statistics(vec![]);
        assert_eq!(stats.mean, 0.0);
        assert_eq!(stats.stddev, 0.0);
        assert_eq!(stats.median, 0.0);
        assert_eq!(stats.p95, 0.0);
        assert_eq!(stats.p99, 0.0);
        assert!(stats.samples.is_empty());
    }

    #[test]
    fn test_compute_statistics_single_sample() {
        let stats = compute_statistics(vec![100.0]);
        assert_eq!(stats.mean, 100.0);
        assert_eq!(stats.stddev, 0.0);
        assert_eq!(stats.median, 100.0);
        assert_eq!(stats.p95, 100.0);
        assert_eq!(stats.p99, 100.0);
    }

    #[test]
    fn test_compute_statistics_multiple_samples() {
        let stats = compute_statistics(vec![10.0, 20.0, 30.0, 40.0, 50.0]);
        assert_eq!(stats.mean, 30.0);
        assert_eq!(stats.median, 30.0);
        assert!(stats.p95 >= 40.0 && stats.p95 <= 50.0);
        assert!(stats.p99 >= 40.0 && stats.p99 <= 50.0);
    }

    #[test]
    fn test_benchmark_result_meets_target() {
        let result = BenchmarkResult {
            id: "B001".to_string(),
            name: "測試".to_string(),
            unit: "ns".to_string(),
            statistics: compute_statistics(vec![100.0, 200.0]),
            comparison: ComparisonTarget::LowerIsBetter,
        };
        assert!(result.meets_target(), "有效結果應達標");
    }

    #[test]
    fn test_benchmark_result_empty_samples_not_meet_target() {
        let result = BenchmarkResult {
            id: "B001".to_string(),
            name: "測試".to_string(),
            unit: "ns".to_string(),
            statistics: compute_statistics(vec![]),
            comparison: ComparisonTarget::LowerIsBetter,
        };
        assert!(!result.meets_target(), "空樣本不應達標");
    }

    #[test]
    fn test_run_all_benchmarks_with_skip_e2e() {
        let config = BenchmarkConfig { sample_count: 3, warmup_rounds: 1 };
        let results = run_all_benchmarks_with_options(&config, true, None);
        // skip_e2e = true → 只運行 B001/B002/B003，跳過 B004
        assert_eq!(results.len(), 3, "跳過 E2E 後應有 3 項");
        assert_eq!(results[0].id, "B001");
        assert_eq!(results[1].id, "B002");
        assert_eq!(results[2].id, "B003");
    }

    #[test]
    fn test_run_all_benchmarks_without_skip_e2e() {
        let config = BenchmarkConfig { sample_count: 3, warmup_rounds: 1 };
        let results = run_all_benchmarks_with_options(&config, false, None);
        // skip_e2e = false → 運行 B001/B002/B003/B004
        assert_eq!(results.len(), 4, "不跳過 E2E 應有 4 項");
        assert_eq!(results[3].id, "B004");
    }

    #[test]
    fn test_progress_callback_invoked() {
        use std::cell::RefCell;

        let config = BenchmarkConfig { sample_count: 2, warmup_rounds: 0 };
        // Fn 閉包不能可變捕獲，用 RefCell 實現內部可變性
        let calls = RefCell::new(Vec::new());
        let results = run_all_benchmarks_with_options(
            &config,
            true,
            Some(&|current, total, name| {
                calls.borrow_mut().push((current, total, name.to_string()));
            }),
        );
        let calls = calls.borrow();
        assert_eq!(calls.len(), results.len(), "回調應每項調用一次");
        assert_eq!(calls[0].0, 1, "第一項 current=1");
        assert_eq!(calls[0].1, 3, "total=3");
    }
}
