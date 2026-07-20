//! # nuzo_signal Performance Benchmark
//!
//! Measures core performance characteristics of the signal-slot system:
//!   S1 - Connect throughput (sequential insertion)
//!   S2 - Emit with no slots (baseline overhead)
//!   S3 - Emit with single slot (callback dispatch)
//!   S4 - Emit with many slots (100 slots, sorted dispatch)
//!   S5 - Disconnect throughput (individual removal)
//!   S6 - Disconnect by group (batch removal)
//!   S7 - Concurrent emit (4 threads, read-heavy)
//!   S8 - SignalBus find (type-erased lookup)
//!
//! # Usage
//!
//! ```bash
//! cargo run -p nuzo_signal --example bench_signal --release
//! ```

use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
use std::thread;
use std::time::Instant;

use nuzo_signal::{BusScope, Signal, SignalBus, SignalKey};

// 基准测试专用 SignalKey 常量
const BENCH_BUS_FIND_KEY: SignalKey<i32> =
    SignalKey::new("bench_bus_find", BusScope::Custom("bench"));

// ============================================================================
// Constants
// ============================================================================

/// Number of warm-up iterations before timing (default).
const DEFAULT_WARMUP_ROUNDS: usize = 100;

/// Number of timed sampling rounds (default).
const DEFAULT_SAMPLE_ROUNDS: usize = 50;

/// Report box width (ASCII characters).
const BOX_WIDTH: usize = 62;

// ============================================================================
// Environment-Overridable Round Counts
// ============================================================================

/// 全局缓存的热身轮数（首次访问时从环境变量读取并缓存）。
///
/// # 环境变量
/// - `NUZO_BENCH_PREHEAT_ROUNDS`：覆盖默认值 100
///   （命名沿用上游任务约定，对应代码中的 WARMUP_ROUNDS 概念）
///
/// # 使用场景
/// CI 中可降低采样轮数加速回归测试：
/// ```powershell
/// $env:NUZO_BENCH_PREHEAT_ROUNDS = "5"
/// $env:NUZO_BENCH_SAMPLE_ROUNDS = "10"
/// cargo run -p nuzo --example bench_signal --release
/// ```
static WARMUP_ROUNDS_CELL: OnceLock<usize> = OnceLock::new();

/// 全局缓存的采样轮数（首次访问时从环境变量读取并缓存）。
///
/// # 环境变量
/// - `NUZO_BENCH_SAMPLE_ROUNDS`：覆盖默认值 50
static SAMPLE_ROUNDS_CELL: OnceLock<usize> = OnceLock::new();

/// 获取热身轮数（WARMUP_ROUNDS）。
///
/// 首次调用时读取环境变量 `NUZO_BENCH_PREHEAT_ROUNDS`，解析失败时回退到默认值。
fn warmup_rounds() -> usize {
    *WARMUP_ROUNDS_CELL.get_or_init(|| {
        std::env::var("NUZO_BENCH_PREHEAT_ROUNDS")
            .ok()
            .and_then(|s| s.parse().ok())
            .filter(|&v: &usize| v > 0)
            .unwrap_or(DEFAULT_WARMUP_ROUNDS)
    })
}

/// 获取采样轮数（SAMPLE_ROUNDS）。
///
/// 首次调用时读取环境变量 `NUZO_BENCH_SAMPLE_ROUNDS`，解析失败时回退到默认值。
fn sample_rounds() -> usize {
    *SAMPLE_ROUNDS_CELL.get_or_init(|| {
        std::env::var("NUZO_BENCH_SAMPLE_ROUNDS")
            .ok()
            .and_then(|s| s.parse().ok())
            .filter(|&v: &usize| v > 0)
            .unwrap_or(DEFAULT_SAMPLE_ROUNDS)
    })
}

// ============================================================================
// Micro Statistics Framework
// ============================================================================

/// Result of a single benchmark run.
struct BenchResult {
    /// Short ID (e.g., "S1").
    id: &'static str,
    /// Human-readable name.
    name: &'static str,
    /// Per-round elapsed time in nanoseconds.
    samples_ns: Vec<f64>,
    /// Number of operations per round.
    ops_per_round: u64,
    /// Unit label for throughput.
    unit: &'static str,
}

impl BenchResult {
    /// Compute statistics from samples.
    fn stats(&self) -> BenchStats {
        let mut sorted = self.samples_ns.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

        let n = sorted.len() as f64;
        let mean = sorted.iter().sum::<f64>() / n;
        let variance = sorted.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / n;
        let stddev = variance.sqrt();

        let p95_idx = ((n * 0.95) as usize).min(sorted.len() - 1);
        let p99_idx = ((n * 0.99) as usize).min(sorted.len() - 1);

        let median = sorted[sorted.len() / 2];
        let p95 = sorted[p95_idx];
        let p99 = sorted[p99_idx];

        // Throughput: ops/s based on mean
        let mean_secs = mean / 1_000_000_000.0;
        let throughput = if mean_secs > 0.0 { self.ops_per_round as f64 / mean_secs } else { 0.0 };

        BenchStats {
            mean_ns: mean,
            median_ns: median,
            stddev_ns: stddev,
            p95_ns: p95,
            p99_ns: p99,
            throughput,
        }
    }
}

/// Computed statistics for a benchmark.
struct BenchStats {
    mean_ns: f64,
    median_ns: f64,
    stddev_ns: f64,
    p95_ns: f64,
    p99_ns: f64,
    throughput: f64,
}

/// Run a benchmark: warm-up, then timed sampling.
///
/// Each round calls `f()` which should execute `ops_per_round` operations.
/// The closure receives the round number for potential variation.
fn bench<F: Fn(usize)>(
    id: &'static str,
    name: &'static str,
    ops_per_round: u64,
    unit: &'static str,
    f: F,
) -> BenchResult {
    // Warm-up
    let warmup = warmup_rounds();
    for i in 0..warmup {
        f(i);
    }

    // Timed sampling
    let sample = sample_rounds();
    let mut samples_ns = Vec::with_capacity(sample);
    for i in 0..sample {
        let start = Instant::now();
        f(i);
        let elapsed = start.elapsed();
        samples_ns.push(elapsed.as_nanos() as f64);
    }

    BenchResult { id, name, samples_ns, ops_per_round, unit }
}

// ============================================================================
// Benchmark Cases
// ============================================================================

/// S1: Connect throughput
fn bench_connect_throughput() -> BenchResult {
    const OPS: u64 = 10_000;
    bench("S1", "Connect Throughput", OPS, "connects/s", |_| {
        let signal: Signal<i32> = Signal::named("bench_connect");
        for _ in 0..OPS {
            // bench 场景：connect 失败直接 panic，避免测量结果失真
            signal.connect(|_| {}).expect("connect failed in bench S1");
        }
    })
}

/// S2: Emit with no slots (baseline overhead)
fn bench_emit_no_slots() -> BenchResult {
    const OPS: u64 = 1_000_000;
    let signal: Signal<i32> = Signal::named("bench_empty");
    bench("S2", "Emit (0 slots)", OPS, "emits/s", |_| {
        for _ in 0..OPS {
            signal.emit(&42);
        }
    })
}

/// S3: Emit with single slot
fn bench_emit_single_slot() -> BenchResult {
    const OPS: u64 = 1_000_000;
    let signal: Signal<i32> = Signal::named("bench_single");
    let counter = Arc::new(AtomicUsize::new(0));
    let c = counter.clone();
    signal
        .connect(move |v| {
            c.fetch_add(*v as usize, AtomicOrdering::Relaxed);
        })
        .expect("connect failed in bench S3");
    bench("S3", "Emit (1 slot)", OPS, "emits/s", |_| {
        for _ in 0..OPS {
            signal.emit(&1);
        }
    })
}

/// S4: Emit with many slots (100)
fn bench_emit_many_slots() -> BenchResult {
    const SLOT_COUNT: usize = 100;
    const OPS: u64 = 100_000;
    let signal: Signal<i32> = Signal::named("bench_many");
    let counter = Arc::new(AtomicUsize::new(0));
    for _ in 0..SLOT_COUNT {
        let c = counter.clone();
        signal
            .connect(move |v| {
                c.fetch_add(*v as usize, AtomicOrdering::Relaxed);
            })
            .expect("connect failed in bench S4");
    }
    bench("S4", "Emit (100 slots)", OPS, "emits/s", |_| {
        for _ in 0..OPS {
            signal.emit(&1);
        }
    })
}

/// S5: Disconnect throughput
fn bench_disconnect_throughput() -> BenchResult {
    const OPS: u64 = 10_000;
    bench("S5", "Disconnect Throughput", OPS, "disconnects/s", |_| {
        let signal: Signal<i32> = Signal::named("bench_disconnect");
        // 预分配容量，避免迭代器链中潜在的 realloc
        // （Range<u64>: ExactSizeIterator 已让 collect 预分配，显式 with_capacity 更清晰）
        let mut connections = Vec::with_capacity(OPS as usize);
        for _ in 0..OPS {
            // bench 场景：connect 失败直接 panic，避免测量结果失真
            connections.push(signal.connect(|_| {}).expect("connect failed in bench S5"));
        }
        for conn in connections {
            conn.disconnect();
        }
    })
}

/// S6: Disconnect by group
fn bench_disconnect_by_group() -> BenchResult {
    const OPS: u64 = 10_000;
    bench("S6", "Disconnect by Group", OPS, "removals/s", |_| {
        let signal: Signal<i32> = Signal::named("bench_group");
        for _ in 0..OPS {
            signal
                .connect_with_group(|_| {}, "test_group")
                .expect("connect_with_group failed in bench S6");
        }
        signal.disconnect_by_group("test_group");
    })
}

/// S7: Concurrent emit (4 threads)
fn bench_concurrent_emit() -> BenchResult {
    const THREADS: usize = 4;
    const OPS_PER_THREAD: u64 = 100_000;
    let total_ops = OPS_PER_THREAD * THREADS as u64;

    bench("S7", "Concurrent Emit (4 threads)", total_ops, "emits/s", |_| {
        let signal: Signal<i32> = Signal::named("bench_concurrent");
        let counter = Arc::new(AtomicUsize::new(0));
        for _ in 0..10 {
            let c = counter.clone();
            signal
                .connect(move |v| {
                    c.fetch_add(*v as usize, AtomicOrdering::Relaxed);
                })
                .expect("connect failed in bench S7");
        }

        let handles: Vec<_> = (0..THREADS)
            .map(|_| {
                let sig = signal.clone_handle();
                thread::spawn(move || {
                    for _ in 0..OPS_PER_THREAD {
                        sig.emit(&1);
                    }
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }
    })
}

/// S8: SignalBus get throughput
fn bench_signal_bus_find() -> BenchResult {
    const OPS: u64 = 1_000_000;

    // Register a signal on the scoped bus
    let bus = SignalBus::scoped(BusScope::Custom("bench"));
    let signal: Signal<i32> = Signal::named("bench_bus_find");
    bus.register(&BENCH_BUS_FIND_KEY, &signal).unwrap();

    bench("S8", "SignalBus Get", OPS, "lookups/s", |_| {
        for _ in 0..OPS {
            let _ = bus.get(&BENCH_BUS_FIND_KEY);
        }
    })
}

// ============================================================================
// Unicode Box-Drawing Report
// ============================================================================

/// Format nanoseconds to human-readable string.
fn fmt_ns(ns: f64) -> String {
    if ns < 1_000.0 {
        format!("{:.1} ns", ns)
    } else if ns < 1_000_000.0 {
        format!("{:.2} us", ns / 1_000.0)
    } else if ns < 1_000_000_000.0 {
        format!("{:.2} ms", ns / 1_000_000.0)
    } else {
        format!("{:.2} s", ns / 1_000_000_000.0)
    }
}

/// Format throughput to human-readable string.
fn fmt_throughput(ops: f64, unit: &str) -> String {
    if ops >= 1_000_000_000.0 {
        format!("{:.2} G{}", ops / 1_000_000_000.0, unit)
    } else if ops >= 1_000_000.0 {
        format!("{:.2} M{}", ops / 1_000_000.0, unit)
    } else if ops >= 1_000.0 {
        format!("{:.2} K{}", ops / 1_000.0, unit)
    } else {
        format!("{:.2} {}", ops, unit)
    }
}

/// Print the full benchmark report with Unicode box-drawing borders.
fn print_report(results: &[BenchResult]) {
    let top = format!("+{}+", "-".repeat(BOX_WIDTH - 2));
    let bottom = format!("+{}+", "-".repeat(BOX_WIDTH - 2));
    let sep = format!("+{}+", "-".repeat(BOX_WIDTH - 2));

    println!("\n{top}");
    println!("| {:<60} |", "nuzo_signal Performance Benchmark");
    println!("{sep}");

    for result in results {
        let stats = result.stats();

        println!("| [{}] {:<54} |", result.id, result.name);
        println!("| {:<60} |", "");

        // Mean / Median
        println!(
            "|   Mean: {:>12}  Median: {:>12}              |",
            fmt_ns(stats.mean_ns),
            fmt_ns(stats.median_ns),
        );

        // P95 / P99
        println!(
            "|   P95:  {:>12}  P99:   {:>12}              |",
            fmt_ns(stats.p95_ns),
            fmt_ns(stats.p99_ns),
        );

        // Stddev
        println!("|   Stddev: {:>12}                                 |", fmt_ns(stats.stddev_ns),);

        // Throughput
        println!("|   Throughput: {:<42}  |", fmt_throughput(stats.throughput, result.unit),);

        println!("| {:<60} |", "");
    }

    // Summary
    println!("{sep}");
    println!("| {:<60} |", "Summary");
    println!("{sep}");

    for result in results {
        let stats = result.stats();
        println!(
            "|   [{}] {:<30} {:>20}  |",
            result.id,
            result.name,
            fmt_throughput(stats.throughput, result.unit),
        );
    }

    println!("{bottom}");
}

// ============================================================================
// Main Entry Point
// ============================================================================

fn main() {
    let overall_start = Instant::now();

    // 显示当前轮数配置（便于 CI 调试）
    eprintln!("  Config: warmup={} rounds, sample={} rounds", warmup_rounds(), sample_rounds(),);

    let results = vec![
        bench_connect_throughput(),
        bench_emit_no_slots(),
        bench_emit_single_slot(),
        bench_emit_many_slots(),
        bench_disconnect_throughput(),
        bench_disconnect_by_group(),
        bench_concurrent_emit(),
        bench_signal_bus_find(),
    ];

    let total_elapsed = overall_start.elapsed();

    print_report(&results);

    eprintln!("\n  Done: {} benchmarks in {:.1}s\n", results.len(), total_elapsed.as_secs_f64(),);
}
