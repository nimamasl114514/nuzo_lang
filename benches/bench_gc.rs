//! # nuzo_vm GC Performance Benchmark
//!
//! Measures core performance characteristics of the garbage collector:
//!   G1 - Pure allocation throughput (no GC trigger)
//!   G2 - Allocation + frequent GC (short-lived objects)
//!   G3 - Deep object graph tracing (1000-node linked list)
//!   G4 - Circular reference handling (a <-> b)
//!   G5 - Mark-only throughput (mark_roots without sweep)
//!   G6 - Large array allocation (100-element arrays)
//!   G7 - Dict allocation throughput
//!   G8 - Incremental GC pacing overhead
//!
//! # Usage
//!
//! ```bash
//! cargo run -p nuzo_vm --example bench_gc --release
//! ```

use std::time::Instant;

use nuzo_core::tag::{GC_MANAGED_BIT, HEAP_INDEX_MASK_NO_GC, HEAP_TAG};
use nuzo_values::{HeapObject, Value};
use nuzo_vm::gc::Gc;

// ============================================================================
// Constants
// ============================================================================

/// Number of warm-up iterations before timing.
const WARMUP_ROUNDS: usize = 50;

/// Number of timed sampling rounds.
const SAMPLE_ROUNDS: usize = 30;

/// Report box width (ASCII characters).
const BOX_WIDTH: usize = 62;

// ============================================================================
// Micro Statistics Framework
// ============================================================================

/// Result of a single benchmark run.
struct BenchResult {
    /// Short ID (e.g., "G1").
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
fn bench<F: Fn(usize)>(
    id: &'static str,
    name: &'static str,
    ops_per_round: u64,
    unit: &'static str,
    f: F,
) -> BenchResult {
    // Warm-up
    for i in 0..WARMUP_ROUNDS {
        f(i);
    }

    // Timed sampling
    let mut samples_ns = Vec::with_capacity(SAMPLE_ROUNDS);
    for i in 0..SAMPLE_ROUNDS {
        let start = Instant::now();
        f(i);
        let elapsed = start.elapsed();
        samples_ns.push(elapsed.as_nanos() as f64);
    }

    BenchResult { id, name, samples_ns, ops_per_round, unit }
}

// ============================================================================
// Helper: Create a heap Value from GC index
// ============================================================================

/// Create a GC-managed Value from a heap index.
fn gc_value(idx: u32) -> Value {
    unsafe {
        Value::from_raw_bits(HEAP_TAG | GC_MANAGED_BIT | (idx as u64 & HEAP_INDEX_MASK_NO_GC))
    }
}

// ============================================================================
// Benchmark Cases
// ============================================================================

/// G1: Pure allocation throughput (large threshold, no GC trigger)
fn bench_alloc_throughput() -> BenchResult {
    const OPS: u64 = 100_000;
    bench("G1", "Pure Alloc Throughput", OPS, "allocs/s", |_| {
        let mut gc = Gc::new(10 * 1024 * 1024); // 10 MB threshold, no GC trigger
        for i in 0..OPS {
            gc.alloc(HeapObject::Array(vec![Value::from_number(i as f64)]));
        }
    })
}

/// G2: Allocation + frequent GC (tiny threshold forces frequent collection)
fn bench_alloc_with_gc() -> BenchResult {
    const OPS: u64 = 100_000;
    bench("G2", "Alloc + Frequent GC", OPS, "alloc+gc/s", |_| {
        let mut gc = Gc::new(4096); // Tiny threshold forces frequent GC
        for i in 0..OPS {
            let idx = gc.alloc(HeapObject::Array(vec![Value::from_number(i as f64)]));
            // Every 10 allocations, mark the latest as root and collect
            if i % 10 == 9 {
                let root = gc_value(idx);
                gc.mark_roots(std::iter::once(root));
                gc.collect();
            }
        }
    })
}

/// G3: Deep object graph tracing (1000-node linked list)
fn bench_deep_graph_trace() -> BenchResult {
    const NODE_COUNT: usize = 1000;
    const OPS: u64 = 1000; // 1000 mark+sweep cycles
    bench("G3", "Deep Graph Trace (1000 nodes)", OPS, "cycles/s", |_| {
        let mut gc = Gc::new(10 * 1024 * 1024);
        // Build linked list: node -> node -> ... -> tail
        let mut last = gc.alloc(HeapObject::Array(vec![]));
        for i in 1..NODE_COUNT as u32 {
            let node =
                gc.alloc(HeapObject::Array(vec![Value::from_number(i as f64), gc_value(last)]));
            last = node;
        }
        let root = gc_value(last);
        for _ in 0..OPS {
            gc.mark_roots(std::iter::once(root));
            gc.collect();
        }
    })
}

/// G4: Circular reference handling (a <-> b)
fn bench_circular_ref() -> BenchResult {
    const OPS: u64 = 10_000;
    bench("G4", "Circular Ref (a<->b)", OPS, "cycles/s", |_| {
        let mut gc = Gc::new(10 * 1024 * 1024);
        let a = gc.alloc(HeapObject::Array(vec![]));
        let b = gc.alloc(HeapObject::Array(vec![gc_value(a)]));
        // Create circular reference: a[0] = b
        if let HeapObject::Array(arr) =
            gc.get_mut(a).expect("gc.get_mut should succeed for valid index")
        {
            arr.push(gc_value(b));
        }
        let root = gc_value(a);
        for _ in 0..OPS {
            gc.mark_roots(std::iter::once(root));
            gc.collect();
        }
    })
}

/// G5: Mark-only throughput (mark_roots without collect)
fn bench_mark_only() -> BenchResult {
    const SLOT_COUNT: usize = 1000;
    const OPS: u64 = 10_000;
    bench("G5", "Mark-Only Throughput", OPS, "marks/s", |_| {
        let mut gc = Gc::new(10 * 1024 * 1024);
        // Pre-allocate objects
        let mut roots = Vec::with_capacity(SLOT_COUNT);
        for i in 0..SLOT_COUNT {
            let idx = gc.alloc(HeapObject::Array(vec![Value::from_number(i as f64)]));
            roots.push(gc_value(idx));
        }
        for _ in 0..OPS {
            gc.mark_roots(roots.iter().copied());
        }
    })
}

/// G6: Large array allocation (100-element arrays)
fn bench_large_array_alloc() -> BenchResult {
    const OPS: u64 = 10_000;
    bench("G6", "Large Array Alloc (100 elems)", OPS, "allocs/s", |_| {
        let mut gc = Gc::new(10 * 1024 * 1024);
        for i in 0..OPS {
            let arr: Vec<Value> =
                (0..100).map(|j| Value::from_number((i * 100 + j) as f64)).collect();
            gc.alloc(HeapObject::Array(arr));
        }
    })
}

/// G7: Dict allocation throughput
fn bench_dict_alloc() -> BenchResult {
    const OPS: u64 = 10_000;
    bench("G7", "Dict Alloc Throughput", OPS, "allocs/s", |_| {
        let mut gc = Gc::new(10 * 1024 * 1024);
        for _ in 0..OPS {
            gc.alloc(HeapObject::Dict(nuzo_values::NuzoDict::new()));
        }
    })
}

/// G8: Incremental GC pacing overhead
fn bench_incremental_pacing() -> BenchResult {
    const OPS: u64 = 100_000;
    bench("G8", "Incremental Pacing Overhead", OPS, "allocs/s", |_| {
        let mut gc = Gc::new(10 * 1024 * 1024);
        // Fill heap to ~50% to trigger incremental pacing
        for i in 0..5000 {
            gc.alloc(HeapObject::Array(vec![Value::from_number(i as f64)]));
        }
        // Now measure alloc with incremental pacing active
        for i in 0..OPS {
            gc.alloc(HeapObject::Array(vec![Value::from_number(i as f64)]));
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

/// Print the full benchmark report with ASCII box-drawing borders.
fn print_report(results: &[BenchResult]) {
    let top = format!("+{}+", "-".repeat(BOX_WIDTH - 2));
    let bottom = format!("+{}+", "-".repeat(BOX_WIDTH - 2));
    let sep = format!("+{}+", "-".repeat(BOX_WIDTH - 2));

    println!("\n{top}");
    println!("| {:<60} |", "nuzo_vm GC Performance Benchmark");
    println!("{sep}");

    for result in results {
        let stats = result.stats();

        println!("| [{}] {:<54} |", result.id, result.name);
        println!("| {:<60} |", "");

        println!(
            "|   Mean: {:>12}  Median: {:>12}              |",
            fmt_ns(stats.mean_ns),
            fmt_ns(stats.median_ns),
        );

        println!(
            "|   P95:  {:>12}  P99:   {:>12}              |",
            fmt_ns(stats.p95_ns),
            fmt_ns(stats.p99_ns),
        );

        println!("|   Stddev: {:>12}                                 |", fmt_ns(stats.stddev_ns),);

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

    let results = vec![
        bench_alloc_throughput(),
        bench_alloc_with_gc(),
        bench_deep_graph_trace(),
        bench_circular_ref(),
        bench_mark_only(),
        bench_large_array_alloc(),
        bench_dict_alloc(),
        bench_incremental_pacing(),
    ];

    let total_elapsed = overall_start.elapsed();

    print_report(&results);

    eprintln!("\n  Done: {} benchmarks in {:.1}s\n", results.len(), total_elapsed.as_secs_f64(),);
}
