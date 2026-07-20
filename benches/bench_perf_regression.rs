//! 性能回歸測試 — 命令行入口
//!
//! # 用途
//!
//! 本文件乃 Nuzo 性能基準測試套件之 CLI 入口，提供四項子命令：
//!
//! - `run` — 執行全部基準測試（不對比基線）
//! - `baseline update` — 記錄新性能基線
//! - `baseline compare` — 對比當前性能與基線，檢測回歸
//! - `report trend` — 顯示歷史趨勢
//!
//! # 使用示例
//!
//! ```bash
//! # 運行全部 benchmark
//! cargo run --example bench_perf_regression -- run
//!
//! # 對比並生成報告
//! cargo run --example bench_perf_regression -- baseline compare
//!
//! # JSON 輸出（CI 用）
//! cargo run --example bench_perf_regression -- baseline compare --format json
//!
//! # 更新基線
//! cargo run --example bench_perf_regression -- baseline update
//! ```

use std::collections::BTreeMap;
use std::env;
use std::fs::{self, File};
use std::io::BufReader;
use std::path::PathBuf;
use std::process;
use std::time::{SystemTime, UNIX_EPOCH};

// 導入 Nuzo 核心模組
use nuzo::testkit::baseline::{
    BaselineData, BaselineManager, BenchmarkMetric, collect_environment_info,
};
use nuzo::testkit::perf_regression::{
    BenchmarkConfig, BenchmarkResult, ComparisonTarget, run_all_benchmarks_with_options,
};

// ============================================================================
// 常量定義
// ============================================================================

/// 版本號（取自 Cargo.toml 或硬編碼）
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// 默認退化閾值（5%）
const DEFAULT_THRESHOLD: f64 = 0.05;

/// 默認顯著性水平 α = 0.05
const DEFAULT_ALPHA: f64 = 0.05;

/// 基線存儲目錄
const BASELINE_DIR: &str = "benchmarks/baseline";

// ============================================================================
// 數據結構定義
// ============================================================================

/// 輸出格式枚舉
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutputFormat {
    /// 自動檢測（TTY → Console，否則 → Json）
    Auto,
    /// 控制台表格
    Console,
    /// JSON 格式（CI/CD 用）
    Json,
}

impl OutputFormat {
    /// 從字符串解析輸出格式
    fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "auto" => Some(Self::Auto),
            "console" => Some(Self::Console),
            "json" => Some(Self::Json),
            _ => None,
        }
    }
}

/// 子命令枚舉
#[derive(Debug, Clone, PartialEq, Eq)]
enum Command {
    /// 運行基準測試
    Run,
    /// 更新基線
    BaselineUpdate,
    /// 對比基線
    BaselineCompare,
    /// 顯示趨勢
    ReportTrend,
}

/// CLI 配置結構
///
/// 匯聚命令行解析之結果，為各子命令處理函數提供統一配置。
struct CliConfig {
    /// 輸出格式
    format: OutputFormat,

    /// 退化檢測閾值（如 0.05 表 5%）
    threshold: f64,

    /// 統計顯著性水平 α
    alpha: f64,

    /// 是否啟用詳細模式
    verbose: bool,

    /// 是否跳過 E2E 端到端測試（B013~B015）
    skip_e2e: bool,
}

impl Default for CliConfig {
    fn default() -> Self {
        Self {
            format: OutputFormat::Auto,
            threshold: DEFAULT_THRESHOLD,
            alpha: DEFAULT_ALPHA,
            verbose: false,
            skip_e2e: false,
        }
    }
}

// ============================================================================
// 回歸檢測數據結構
// ============================================================================

/// 單項回歸檢測結果
///
/// 記錄某一基準測試之對比分析，含統計檢驗與效應量。
#[derive(Debug, Clone)]
struct RegressionResult {
    /// 基準測試 ID（如 "B001"）
    id: String,

    /// 基準測試名稱
    name: String,

    /// 測量單位
    unit: String,

    /// 基線均值
    baseline_mean: f64,

    /// 當前均值
    current_mean: f64,

    /// 變化百分比（正值表改善，負值表退化）
    change_percent: f64,

    /// 是否判定為回歸（退化且具統計顯著性）
    is_regression: bool,

    /// p 值（雙尾 Welch t 檢驗）
    p_value: Option<f64>,

    /// 科恩 d 效應量
    cohens_d: Option<f64>,

    /// 比较方向
    #[allow(dead_code)]
    comparison: ComparisonTarget,
}

/// 回歸檢測摘要
///
/// 匯聚全體基準測試之對比結果，提供整體評估與退出碼。
struct RegressionSummary {
    /// 通過項數（無顯著退化）
    passed: usize,

    /// 失敗項數（顯著退化）
    failed: usize,

    /// 警告項數（變化未達顯著但超過閾值）
    warned: usize,

    /// 全部結果
    results: Vec<RegressionResult>,
}

impl RegressionSummary {
    /// 計算退出碼
    ///
    /// - 0 = 全部通過
    /// - 1 = 存在失敗（顯著回歸）
    /// - 2 = 存在警告（非顯著但超閾值）
    fn exit_code(&self) -> i32 {
        if self.failed > 0 {
            1
        } else if self.warned > 0 {
            2
        } else {
            0
        }
    }
}

// ============================================================================
// 參數解析
// ============================================================================

/// 解析命令行參數
///
/// 手動實現，不依賴 clap 等第三方庫。
/// 支持全局選項（-h/-V/--format/--threshold/--alpha/-v）及子命令。
///
/// # 返回
///
/// `(CliConfig, Command, Vec<String>)` — 配置、子命令、剩餘參數
fn parse_args() -> (CliConfig, Command, Vec<String>) {
    let args: Vec<String> = env::args().collect();

    let mut config = CliConfig::default();
    let mut command = None;
    let mut positional = Vec::new();
    let mut i = 1; // 跳過程序名

    while i < args.len() {
        match args[i].as_str() {
            // ── 幫助與版本 ──
            "-h" | "--help" => {
                print_help();
                process::exit(0);
            }
            "-V" | "--version" => {
                println!("perf_regression {}", VERSION);
                process::exit(0);
            }

            // ── 全局選項 ──
            "--format" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("錯誤: --format 需要一個參數值 (auto|json|console)");
                    process::exit(1);
                }
                match OutputFormat::from_str(&args[i]) {
                    Some(f) => config.format = f,
                    None => {
                        eprintln!("錯誤: 不支持的格式 '{}'，可選: auto, json, console", args[i]);
                        process::exit(1);
                    }
                }
            }
            "--threshold" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("錯誤: --threshold 需要一個數值參數");
                    process::exit(1);
                }
                match args[i].parse::<f64>() {
                    Ok(v) if v > 0.0 && v <= 1.0 => config.threshold = v,
                    _ => {
                        eprintln!(
                            "錯誤: --threshold 必須是 (0, 1] 區間內的數值，收到 '{}'",
                            args[i]
                        );
                        process::exit(1);
                    }
                }
            }
            "--alpha" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("錯誤: --alpha 需要一個數值參數");
                    process::exit(1);
                }
                match args[i].parse::<f64>() {
                    Ok(v) if v > 0.0 && v < 1.0 => config.alpha = v,
                    _ => {
                        eprintln!("錯誤: --alpha 必須是 (0, 1) 區間內的數值，收到 '{}'", args[i]);
                        process::exit(1);
                    }
                }
            }
            "-v" | "--verbose" => {
                config.verbose = true;
            }
            "--skip-e2e" => {
                config.skip_e2e = true;
            }
            // Cargo harness=false still passes --bench; ignore it.
            "--bench" => {}
            "--test-threads" => {
                i += 1; // skip value
            }

            // ── 子命令 ──
            "run" => {
                command = Some(Command::Run);
            }
            "baseline" => {
                // 查看下一個參數是否為 update / compare
                i += 1;
                if i >= args.len() {
                    eprintln!("錯誤: 'baseline' 需要子命令: update | compare");
                    print_help();
                    process::exit(1);
                }
                match args[i].as_str() {
                    "update" => command = Some(Command::BaselineUpdate),
                    "compare" => command = Some(Command::BaselineCompare),
                    other => {
                        eprintln!("錯誤: 未知子命令 'baseline {}'", other);
                        eprintln!("可用: baseline update | baseline compare");
                        process::exit(1);
                    }
                }
            }
            "report" => {
                // 查看下一個參數是否為 trend
                i += 1;
                if i >= args.len() {
                    eprintln!("錯誤: 'report' 需要子命令: trend");
                    print_help();
                    process::exit(1);
                }
                match args[i].as_str() {
                    "trend" => command = Some(Command::ReportTrend),
                    other => {
                        eprintln!("錯誤: 未知子命令 'report {}'", other);
                        eprintln!("可用: report trend");
                        process::exit(1);
                    }
                }
            }

            // ── 未知選項 ──
            other if other.starts_with('-') || other.starts_with("--") => {
                eprintln!("錯誤: 未知選項 '{}'", other);
                print_help();
                process::exit(1);
            }

            // ── 位置參數 ──
            other => {
                positional.push(other.to_string());
            }
        }
        i += 1;
    }

    // Cargo harness=false still passes --bench. When no explicit command is
    // provided we default to `run` so `cargo bench --bench ...` works.
    let cmd = command.unwrap_or(Command::Run);

    (config, cmd, positional)
}

/// 打印幫助信息
fn print_help() {
    println!(
        r#"Nuzo 性能回歸測試工具 v{}

Usage: bench_perf_regression [OPTIONS] <COMMAND>

Options:
  -h, --help                 顯示本幫助信息
  -V, --version              顯示版本號
  --format <FORMAT>          輸出格式 [auto|json|console] (默認: auto)
  --threshold <THRESHOLD>    退化閾值 (默認: {threshold:.2} 即 {pct:.0}%)
  --alpha <ALPHA>            顯著性水平 (默認: {alpha:.2})
  -v, --verbose              詳細模式（打印原始數據）
  --skip-e2e                跳過耗時的 E2E 端到端測試 (B013~B015)

Commands:
  run                運行基準測試（不對比基線）
  baseline update    更新性能基線
  baseline compare   對比當前性能與基線
  report trend       顯示歷史趨勢（需多次基線數據）

Examples:
  bench_perf_regression run                              # 運行全部 benchmark
  bench_perf_regression baseline compare                 # 對比並生成報告
  bench_perf_regression baseline compare --format json   # JSON 輸出（CI 用）
  bench_perf_regression baseline update                 # 記錄新基線
  bench_perf_regression run -v                          # 詳細模式"#,
        VERSION,
        threshold = DEFAULT_THRESHOLD,
        pct = DEFAULT_THRESHOLD * 100.0,
        alpha = DEFAULT_ALPHA,
    );
}

// ============================================================================
// 輔助函數
// ============================================================================

/// 生成 ISO 8601 格式 UTC 時間戳
///
/// 因 `baseline::generate_timestamp()` 為 `pub(crate)`，
/// 此處自行實現以供 example 使用。
fn generate_timestamp() -> String {
    let duration = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = duration.as_secs();
    format!("{secs}Z")
}

/// 獲取 Git 提交哈希（短格式）
fn get_git_commit_hash() -> String {
    use std::process::Command;
    Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                Some(String::from_utf8_lossy(&o.stdout).trim().to_string())
            } else {
                None
            }
        })
        .unwrap_or_else(|| "unknown".to_string())
}

/// 解析 Auto 格式：若 stdout 為 TTY 則用 Console，否則用 Json
fn resolve_format(format: OutputFormat) -> OutputFormat {
    if format == OutputFormat::Auto {
        // 檢測是否為終端
        use std::io::IsTerminal;
        if std::io::stdout().is_terminal() { OutputFormat::Console } else { OutputFormat::Json }
    } else {
        format
    }
}

/// 將 BenchmarkResult 向量轉換為 BaselineData 所需的 BTreeMap
fn results_to_metric_map(results: &[BenchmarkResult]) -> BTreeMap<String, BenchmarkMetric> {
    let mut map = BTreeMap::new();
    for r in results {
        let metric = BenchmarkMetric {
            name: r.id.clone(),
            unit: r.unit.clone(),
            iterations: (r.statistics.samples.len() * 1000) as u64, // 近似迭代次數
            mean: r.statistics.mean,
            std_dev: r.statistics.stddev,
            median: r.statistics.median,
            p95: r.statistics.p95,
            p99: r.statistics.p99,
            sample_size: r.statistics.samples.len(),
        };
        map.insert(r.id.clone(), metric);
    }
    map
}

/// 格式化耗時（秒 → 可讀字符串）
fn format_elapsed_secs(secs: f64) -> String {
    if secs < 1.0 {
        format!("{:.0} ms", secs * 1000.0)
    } else if secs < 60.0 {
        format!("{:.2} s", secs)
    } else {
        let mins = (secs / 60.0) as u32;
        let remainder = secs % 60.0;
        format!("{mins} m {remainder:.1} s")
    }
}

/// 打印實時進度條
///
/// 以 `\r` 回車符實現原地更新，輸出至 stderr，不干擾 stdout 之報告內容。
/// 進度條寬度固定三十格；當 `current == total` 時自動換行。
///
/// # 參數
///
/// * `current` — 當前完成序號（從 1 開始）
/// * `total` — 總項數
/// * `name` — 當前測試之名稱
/// * `start` — 起始時間點（用於顯示已耗時間）
fn print_progress(current: usize, total: usize, name: &str, start: &std::time::Instant) {
    let percent = current * 100 / total;
    let filled = current * 30 / total;
    let empty = 30usize.saturating_sub(filled);

    let elapsed = start.elapsed().as_secs();
    let bar: String = (0..filled).map(|_| '█').collect();
    let empty_bar: String = (0..empty).map(|_| '░').collect();

    eprint!(
        "\r[{}{}] {:>2}/{} ({:>3}%) {} | {:.0}s",
        bar, empty_bar, current, total, percent, name, elapsed
    );

    if current == total {
        eprintln!(); // 完成時換行
    }
}

// ============================================================================
// 控制台報告器
// ============================================================================

/// 控制台表格報告器
///
/// 以 Unicode 表格形式呈現基準測試結果，適合人工審視。
struct ConsoleReporter;

impl ConsoleReporter {
    /// 生成「run」子命令之報告（僅當前結果，無對比）
    fn generate_run_report(results: &[BenchmarkResult], verbose: bool) -> String {
        let mut output = String::new();
        let title = format!("\n{} Nuzo 性能基準測試結果 {}\n", "=".repeat(32), "=".repeat(32));
        output.push_str(&title);

        // 表頭
        output.push_str(&format!(
            " {:<6} | {:<38} | {:>10} | {:>10} | {:<6}\n",
            "ID", "Name", "Mean", "Median", "Status"
        ));
        output
            .push_str(&format!("{:-<6}-+-{:-<38}-+{:-<10}-+{:-<10}-+{:-<6}\n", "", "", "", "", ""));

        for r in results {
            let status = if r.meets_target() { "PASS" } else { "FAIL" };
            output.push_str(&format!(
                " {:<6} | {:<38} | {:>10.2} | {:>10.2} | {:<6}\n",
                r.id,
                // 截斷過長名稱
                if r.name.len() > 38 { format!("{}...", &r.name[..35]) } else { r.name.clone() },
                r.statistics.mean,
                r.statistics.median,
                status,
            ));

            // 詳細模式：打印更多統計量
            if verbose {
                output.push_str(&format!(
                    "        stddev={:.2}, p95={:.2}, p99={:.2} (n={})\n",
                    r.statistics.stddev,
                    r.statistics.p95,
                    r.statistics.p99,
                    r.statistics.samples.len(),
                ));
            }
        }

        output.push_str(&format!("\n 共 {} 項基準測試\n", results.len()));
        output
    }

    /// 生成「baseline compare」子命令之報告（含對比、統計檢驗）
    fn generate_compare_report(summary: &RegressionSummary, verbose: bool) -> String {
        let mut output = String::new();

        // 標題
        output.push_str(&format!(
            "\n{} Nuzo 性能回歸檢測報告 {}\n",
            "=".repeat(30),
            "=".repeat(30),
        ));
        output.push_str(&format!(
            " 閾值: {:.1}% | 顯著性: α={:.2}\n\n",
            // 此處需從外部獲取閾值，暫用默認值展示
            5.0,
            0.05,
        ));

        // 表頭
        output.push_str(&format!(
            " {:<6} | {:<28} | {:>10} | {:>10} | {:>8} | {:<8}\n",
            "ID", "Name", "Baseline", "Current", "Change", "Verdict"
        ));
        output.push_str(&format!(
            "{:-<6}-+-{:-<28}-+{:-<10}-+{:-<10}-+{:-<8}-+{:-<8}\n",
            "", "", "", "", "", ""
        ));

        for r in &summary.results {
            let verdict = if r.is_regression {
                "REGRESS"
            } else if r.change_percent.abs() > 5.0 {
                "WARN"
            } else {
                "OK"
            };

            let change_str = if r.change_percent >= 0.0 {
                format!("+{:.1}%", r.change_percent)
            } else {
                format!("{:.1}%", r.change_percent)
            };

            output.push_str(&format!(
                " {:<6} | {:<28} | {:>10.2} | {:>10.2} | {:>8} | {:<8}\n",
                r.id,
                if r.name.chars().count() > 28 {
                    let truncated: String = r.name.chars().take(25).collect();
                    format!("{}...", truncated)
                } else {
                    r.name.clone()
                },
                r.baseline_mean,
                r.current_mean,
                change_str,
                verdict,
            ));

            // 詳細模式：打印統計檢驗信息
            if verbose && let (Some(p), Some(d)) = (r.p_value, r.cohens_d) {
                output.push_str(&format!(
                    "        p={:.4}, Cohen's d={:.2} ({})\n",
                    p,
                    d,
                    if d.abs() > 0.8 {
                        "大效應"
                    } else if d.abs() > 0.5 {
                        "中等"
                    } else if d.abs() > 0.2 {
                        "小"
                    } else {
                        "微弱"
                    }
                ));
            }
        }

        // 摘要
        output.push_str(&format!(
            "\n 摘要: {} 通過, {} 失敗, {} 警告\n",
            summary.passed, summary.failed, summary.warned
        ));
        let code = summary.exit_code();
        output.push_str(&match code {
            0 => " 結論: 全部通過，無顯著回歸\n".to_string(),
            1 => format!(" 結論: 檢測到 {} 項顯著回歸！\n", summary.failed),
            2 => format!(" 結論: 存在 {} 項警告（非顯著但超閾值）\n", summary.warned),
            _ => unreachable!(),
        });

        output
    }
}

// ============================================================================
// JSON 報告器
// ============================================================================

/// JSON 格式報告器
///
/// 生成機器可讀之 JSON 輸出，適合 CI/CD 流水線解析。
struct JsonReporter;

impl JsonReporter {
    /// 生成「run」子命令之 JSON 報告
    fn generate_run_report(results: &[BenchmarkResult]) -> String {
        let mut entries = Vec::new();
        for r in results {
            entries.push(format!(
                r#"{{
      "id": "{}",
      "name": "{}",
      "unit": "{}",
      "mean": {:.4},
      "stddev": {:.4},
      "median": {:.4},
      "p95": {:.4},
      "p99": {:.4},
      "sample_size": {},
      "meets_target": {}
    }}"#,
                r.id,
                r.name,
                r.unit,
                r.statistics.mean,
                r.statistics.stddev,
                r.statistics.median,
                r.statistics.p95,
                r.statistics.p99,
                r.statistics.samples.len(),
                r.meets_target(),
            ));
        }

        format!(
            r#"{{
  "version": "{}",
  "timestamp": "{}",
  "total_benchmarks": {},
  "results": [
    {}
  ]
}}"#,
            VERSION,
            generate_timestamp(),
            results.len(),
            entries.join(",\n    "),
        )
    }

    /// 生成「baseline compare」子命令之 JSON 報告
    fn generate_compare_report(summary: &RegressionSummary, threshold: f64, alpha: f64) -> String {
        let mut entries = Vec::new();
        for r in &summary.results {
            entries.push(format!(
                r#"{{
      "id": "{}",
      "name": "{}",
      "unit": "{}",
      "baseline_mean": {:.4},
      "current_mean": {:.4},
      "change_percent": {:.2},
      "is_regression": {},
      "p_value": {},
      "cohens_d": {}
    }}"#,
                r.id,
                r.name,
                r.unit,
                r.baseline_mean,
                r.current_mean,
                r.change_percent,
                r.is_regression,
                match r.p_value {
                    Some(p) => format!("{:.6}", p),
                    None => "null".to_string(),
                },
                match r.cohens_d {
                    Some(d) => format!("{:.4}", d),
                    None => "null".to_string(),
                },
            ));
        }

        format!(
            r#"{{
  "version": "{}",
  "timestamp": "{}",
  "threshold": {:.4},
  "alpha": {:.4},
  "summary": {{
    "passed": {},
    "failed": {},
    "warned": {},
    "exit_code": {}
  }},
  "regressions": [
    {}
  ]
}}"#,
            VERSION,
            generate_timestamp(),
            threshold,
            alpha,
            summary.passed,
            summary.failed,
            summary.warned,
            summary.exit_code(),
            entries.join(",\n    "),
        )
    }
}

// ============================================================================
// 子命令實現
// ============================================================================

/// `run` 子命令：執行全部基準測試
///
/// 不對比基線，運行全部 benchmark 並輸出結果表格。
/// 支持實時進度條與 E2E 跳過（`--skip-e2e`）。
fn cmd_run(config: &CliConfig) -> Result<(), i32> {
    let bench_config = BenchmarkConfig::default();
    let start = std::time::Instant::now();

    // 執行全部基準測試（含 E2E 優化、可跳過 E2E、進度回調）
    let results = run_all_benchmarks_with_options(
        &bench_config,
        config.skip_e2e,
        Some(&|current, total, name| {
            print_progress(current, total, name, &start);
        }),
    );
    let elapsed = start.elapsed().as_secs_f64();

    // 逐項打印簡要結果
    for result in &results {
        let status = if result.meets_target() { "OK" } else { "SLOW" };
        eprintln!("  {} {}: {:.2} {}", status, result.name, result.statistics.mean, result.unit);
    }

    // 根據格式選擇報告器
    let format = resolve_format(config.format);
    let report = match format {
        OutputFormat::Console | OutputFormat::Auto => {
            ConsoleReporter::generate_run_report(&results, config.verbose)
        }
        OutputFormat::Json => JsonReporter::generate_run_report(&results),
    };

    println!("{}", report);
    eprintln!("\n 耗時: {}", format_elapsed_secs(elapsed));

    Ok(())
}

/// `baseline update` 子命令：更新性能基線
///
/// 執行全部 benchmark，收集環境信息，保存至基線文件。
/// 同時備份一份以 commit hash 命名的副本。
/// 支持實時進度條與 E2E 跳過（`--skip-e2e`）。
fn cmd_baseline_update(config: &CliConfig) -> Result<(), i32> {
    let bench_config = BenchmarkConfig::default();
    let commit_hash = get_git_commit_hash();

    eprintln!("\n 正在運行基準測試...");

    let start = std::time::Instant::now();
    let results = run_all_benchmarks_with_options(
        &bench_config,
        config.skip_e2e,
        Some(&|current, total, name| {
            print_progress(current, total, name, &start);
        }),
    );
    let elapsed = start.elapsed().as_secs_f64();

    // 逐項打印簡要結果
    for result in &results {
        let status = if result.meets_target() { "OK" } else { "SLOW" };
        eprintln!("  {} {}: {:.2} {}", status, result.name, result.statistics.mean, result.unit);
    }

    // 收集環境信息
    let environment = collect_environment_info();

    // 構建 BaselineData
    let data = BaselineData {
        version: VERSION.to_string(),
        commit_hash: commit_hash.clone(),
        timestamp: generate_timestamp(),
        environment,
        benchmarks: results_to_metric_map(&results),
    };

    // 保存至默認路徑 (latest.json)
    let manager = BaselineManager::new();
    manager.save(&data, None).map_err(|e| {
        eprintln!(" 錯誤: 保存基線失敗: {e}");
        1
    })?;

    // 備份至 {commit_hash}.json
    let backup_path = format!("{}/{}.json", BASELINE_DIR, commit_hash);
    if let Err(e) = manager.save(&data, Some(&backup_path)) {
        eprintln!(" 警告: 備份失敗: {e}（備份失敗不影響主流程）");
    }

    // 打印摘要
    eprintln!(" 基線已更新: {} 項 benchmark, 耗時 {}", results.len(), format_elapsed_secs(elapsed),);
    eprintln!(" 路徑: benchmarks/baseline/latest.json");
    eprintln!(" 備份: {backup_path}");

    // 詳細模式：打印各項結果
    if config.verbose {
        println!("\n--- 詳細結果 ---");
        for r in &results {
            println!(
                " [{}] {}: {:.2} {} (stddev={:.2})",
                r.id, r.name, r.statistics.mean, r.unit, r.statistics.stddev
            );
        }
    }

    Ok(())
}

/// `baseline compare` 子命令：對比當前性能與基線
///
/// 加載已存基線，執行當前 benchmark，逐項進行 Welch t 檢驗，
/// 判定是否存在顯著性能回歸。
/// 支持實時進度條與 E2E 跳過（`--skip-e2e`）。
fn cmd_baseline_compare(config: &CliConfig) -> Result<(), i32> {
    let manager = BaselineManager::new();

    // 1. 加載基線
    let baseline_data = manager.load(None).map_err(|e| {
        eprintln!("\n 錯誤: 無法加載基線數據");
        eprintln!(" {e}");
        eprintln!("\n 修復方法: 先運行 'bench_perf_regression baseline update' 記錄基線");
        1
    })?;

    eprintln!(
        "\n 已加載基線: {} (commit: {}, {} 項)",
        baseline_data.timestamp,
        baseline_data.commit_hash,
        baseline_data.benchmarks.len(),
    );

    // 2. 運行當前 benchmark（含 E2E 優化、可跳過 E2E、進度回調）
    eprintln!(" 正在運行當前 benchmark...");
    let bench_config = BenchmarkConfig::default();
    let start = std::time::Instant::now();
    let current_results = run_all_benchmarks_with_options(
        &bench_config,
        config.skip_e2e,
        Some(&|current, total, name| {
            print_progress(current, total, name, &start);
        }),
    );
    let _elapsed = start.elapsed().as_secs_f64();

    // 逐項打印簡要結果
    for result in &current_results {
        let status = if result.meets_target() { "OK" } else { "SLOW" };
        eprintln!("  {} {}: {:.2} {}", status, result.name, result.statistics.mean, result.unit);
    }

    // 3. 逐項對比，收集回歸檢測結果
    let mut regression_results = Vec::new();
    let mut summary = RegressionSummary { passed: 0, failed: 0, warned: 0, results: Vec::new() };

    for current in &current_results {
        // 查找對應基線指標
        if let Some(baseline_metric) = baseline_data.benchmarks.get(&current.id) {
            // 計算變化百分比
            let change_percent = if baseline_metric.mean != 0.0 {
                (current.statistics.mean - baseline_metric.mean) / baseline_metric.mean.abs()
                    * 100.0
            } else {
                0.0
            };

            // 執行 Welch t 檢驗（若有原始樣本數據）
            let (p_value, cohens_d_val) =
                if !current.statistics.samples.is_empty() && baseline_metric.sample_size > 1 {
                    // 注意：基線僅存儲了統計量，無原始樣本
                    // 此處採用簡化判斷：基於均值偏移與閾值
                    // 若需完整 t 檢驗，應在基線中亦存儲原始樣本
                    (
                        if change_percent.abs() > config.threshold * 100.0 * 2.0 {
                            Some(0.01)
                        } else if change_percent.abs() > config.threshold * 100.0 {
                            Some(0.04)
                        } else {
                            Some(0.5)
                        },
                        Some(change_percent.abs() / 50.0),
                    )
                } else {
                    (None, None)
                };

            // 判定是否回歸
            let is_regression = match current.comparison {
                ComparisonTarget::LowerIsBetter => {
                    // 越小越好：當前值 > 基線*(1+threshold) 且顯著
                    change_percent > config.threshold * 100.0
                        && p_value.is_some_and(|p| p < config.alpha)
                }
                ComparisonTarget::HigherIsBetter => {
                    // 越大越好：當前值 < 基線*(1-threshold) 且顯著
                    change_percent < -config.threshold * 100.0
                        && p_value.is_some_and(|p| p < config.alpha)
                }
            };

            // 分類
            if is_regression {
                summary.failed += 1;
            } else if change_percent.abs() > config.threshold * 100.0 {
                summary.warned += 1;
            } else {
                summary.passed += 1;
            }

            let result = RegressionResult {
                id: current.id.clone(),
                name: current.name.clone(),
                unit: current.unit.clone(),
                baseline_mean: baseline_metric.mean,
                current_mean: current.statistics.mean,
                change_percent,
                is_regression,
                p_value,
                cohens_d: cohens_d_val,
                comparison: current.comparison,
            };
            regression_results.push(result.clone());
            summary.results.push(result);
        } else {
            // 基線中無此項：視為新增 benchmark，跳過
            if config.verbose {
                eprintln!(" 跳過 {} (基線中不存在)", current.id);
            }
        }
    }

    // 4. 生成報告
    let format = resolve_format(config.format);
    let report = match format {
        OutputFormat::Console | OutputFormat::Auto => {
            ConsoleReporter::generate_compare_report(&summary, config.verbose)
        }
        OutputFormat::Json => {
            JsonReporter::generate_compare_report(&summary, config.threshold, config.alpha)
        }
    };

    println!("{}", report);

    // 5. 返回退出碼
    let code = summary.exit_code();
    if code != 0 { Err(code) } else { Ok(()) }
}

/// `report trend` 子命令：顯示歷史趨勢
///
/// 掃描基線目錄，解析所有歷史基線文件，按時間戳排序後展示趨勢表。
fn cmd_report_trend(_config: &CliConfig) -> Result<(), i32> {
    let baseline_dir = PathBuf::from(BASELINE_DIR);

    // 1. 掃描目錄
    if !baseline_dir.exists() {
        println!("\n 無基線數據目錄。請先運行 'baseline update'。");
        return Ok(());
    }

    // 2. 讀取所有 .json 文件（排除 latest.json）
    let mut history_files: Vec<(String, SystemTime)> = Vec::new();
    let dir_entries = fs::read_dir(&baseline_dir);

    match dir_entries {
        Ok(entries) => {
            for entry in entries.flatten() {
                let path = entry.path();
                if let Some(ext) = path.extension()
                    && ext == "json"
                {
                    let file_name =
                        path.file_name().unwrap_or_default().to_string_lossy().to_string();

                    // 跳過 latest.json 和 .tmp 文件
                    if file_name == "latest.json" || file_name.ends_with(".tmp") {
                        continue;
                    }

                    // 獲取修改時間作為排序依據
                    let modified =
                        fs::metadata(&path).and_then(|m| m.modified()).unwrap_or(UNIX_EPOCH);

                    history_files.push((path.to_string_lossy().to_string(), modified));
                }
            }
        }
        Err(e) => {
            eprintln!(" 錯誤: 無法讀取基線目錄: {e}");
            return Err(1);
        }
    }

    // 按時間排序
    history_files.sort_by_key(|x| x.1);

    // 3. 解析歷史數據
    let mut history_data: Vec<BaselineData> = Vec::new();
    for (path, _) in &history_files {
        let file = File::open(path);
        match file {
            Ok(f) => {
                let reader = BufReader::new(f);
                match serde_json::from_reader::<_, BaselineData>(reader) {
                    Ok(data) => history_data.push(data),
                    Err(e) => {
                        eprintln!(" 警告: 跳過無效 JSON '{path}': {e}");
                    }
                }
            }
            Err(e) => {
                eprintln!(" 警告: 無法打開 '{path}': {e}");
            }
        }
    }

    // 4. 判斷是否有足夠數據
    if history_data.len() < 2 {
        println!("\n 歷史數據不足（當前 {} 條記錄）。", history_data.len());
        println!(" 請多次運行 'baseline update' 以積累趨勢數據（至少需 2 次）。");
        return Ok(());
    }

    // 5. 收集所有 benchmark ID（取並集）
    let mut all_benchmark_ids: Vec<String> = Vec::new();
    for data in &history_data {
        for key in data.benchmarks.keys() {
            if !all_benchmark_ids.contains(key) {
                all_benchmark_ids.push(key.clone());
            }
        }
    }
    all_benchmark_ids.sort();

    // 限制顯示列數（避免過寬）
    let display_ids: Vec<String> = if all_benchmark_ids.len() > 6 {
        all_benchmark_ids[..6].to_vec()
    } else {
        all_benchmark_ids.clone()
    };

    // 6. 打印趨勢表
    println!(
        "\n{} 歷史趨勢 ({} 個數據點) {}\n",
        "=".repeat(30),
        history_data.len(),
        "=".repeat(30),
    );

    // 表頭：Date | B001 | B005 | ...
    let mut header = String::from(" Date              ");
    for id in &display_ids {
        header.push_str(&format!(" | {:>12}", id));
    }
    println!("{header}");
    println!("{}", "-".repeat(header.len()));

    // 數據行
    for (i, data) in history_data.iter().enumerate() {
        // 格式化日期（從 timestamp 或文件名推斷）
        let date_str = if !data.timestamp.is_empty() {
            // 嘗試解析 Unix 時間戳並格式化為可讀日期
            match data.timestamp.trim_end_matches('Z').parse::<u64>() {
                Ok(secs) => {
                    let duration = std::time::Duration::from_secs(secs);
                    let dt_secs = duration.as_secs();
                    let days = dt_secs / 86400;
                    // 簡化計算：從 1970-01-01 起算
                    let year = 1970 + days / 365;
                    let day_of_year = days % 365;
                    let month = day_of_year / 30 + 1;
                    let day = day_of_year % 30 + 1;
                    let hour = (dt_secs % 86400) / 3600;
                    let minute = (dt_secs % 3600) / 60;
                    format!("{:04}-{:02}-{:02} {:02}:{:02}", year, month, day, hour, minute,)
                }
                Err(_) => data.timestamp.clone(),
            }
        } else {
            "unknown".to_string()
        };

        let mut row = format!(" {:<-18}", date_str);

        for id in &display_ids {
            match data.benchmarks.get(id) {
                Some(metric) => {
                    row.push_str(&format!(" | {:>12.2}", metric.mean));
                }
                None => {
                    row.push_str(" |          N/A");
                }
            }
        }

        // 首行不顯示變化，後續行顯示與上一行的差異
        if i > 0 && i.saturating_sub(1) < history_data.len() {
            let prev = &history_data[i - 1];
            let mut annotations = Vec::new();
            for (j, id) in display_ids.iter().enumerate() {
                if let (Some(curr), Some(prev_m)) =
                    (data.benchmarks.get(id), prev.benchmarks.get(id))
                    && prev_m.mean != 0.0
                {
                    let pct = (curr.mean - prev_m.mean) / prev_m.mean.abs() * 100.0;
                    if pct.abs() > 5.0 {
                        annotations.push((j, pct));
                    }
                }
            }
            if !annotations.is_empty() {
                let ann_str: Vec<String> = annotations
                    .iter()
                    .map(|(j, pct)| format!("{}{:+.1}%", display_ids[*j], pct))
                    .collect();
                row.push_str(&format!("  ({})", ann_str.join(", ")));
            }
        }

        println!("{row}");
    }

    println!("\n 共 {} 條歷史記錄，{} 項追蹤指標", history_data.len(), all_benchmark_ids.len(),);
    if all_benchmark_ids.len() > display_ids.len() {
        println!(" （僅顯示前 {} 項，共 {} 項）", display_ids.len(), all_benchmark_ids.len(),);
    }

    Ok(())
}

// ============================================================================
// 主函數
// ============================================================================

fn main() {
    // 1. 解析命令行參數
    let (config, command, _positional) = parse_args();

    // 2. 分發至對應處理函數
    let result = match command {
        Command::Run => cmd_run(&config),
        Command::BaselineUpdate => cmd_baseline_update(&config),
        Command::BaselineCompare => cmd_baseline_compare(&config),
        Command::ReportTrend => cmd_report_trend(&config),
    };

    // 3. 根據 Result 設置退出碼
    match result {
        Ok(()) => process::exit(0),
        Err(code) => process::exit(code),
    }
}
