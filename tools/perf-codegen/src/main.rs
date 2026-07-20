//! Precise codegen performance benchmark for Nuzo Lang.
//!
//! Measures compile time (parse + IR + codegen) using Engine::compile(),
//! which isolates compilation from VM execution and process startup.

use nuzo_run::Engine;
use std::time::Instant;

fn make_array_code(n: usize) -> String {
    let elements: Vec<String> = (0..n).map(|i| i.to_string()).collect();
    format!("len([{}])", elements.join(","))
}

fn benchmark_compile(name: &str, code: &str, engine: &Engine, runs: usize) {
    // Warmup run (first compile may be slower due to allocation)
    let _ = engine.compile(code);

    let mut times = Vec::with_capacity(runs);
    for _ in 0..runs {
        let start = Instant::now();
        match engine.compile(code) {
            Ok(_chunk) => {
                let elapsed = start.elapsed();
                times.push(elapsed.as_secs_f64() * 1000.0); // ms
            }
            Err(e) => {
                eprintln!("ERROR compiling {}: {}", name, e);
                return;
            }
        }
    }

    times.sort_by(|a, b| a.partial_cmp(b).unwrap());

    let min = times[0];
    let median = times[times.len() / 2];
    let mean = times.iter().sum::<f64>() / times.len() as f64;
    let max = times[times.len() - 1];

    println!(
        "{}: runs={} min={:.3}ms median={:.3}ms mean={:.3}ms max={:.3}ms",
        name,
        times.len(),
        min,
        median,
        mean,
        max
    );
}

fn main() {
    let engine = Engine::builder().with_default_config().build().expect("Failed to build engine");

    println!("=== Nuzo Codegen Performance Benchmark ===");
    println!("(measures compile time = parse + IR + codegen, excludes VM execution)");
    println!();

    // Baseline: small array for reference
    let code_small = make_array_code(10);
    benchmark_compile("N=10   array", &code_small, &engine, 20);

    // N=100 array
    let code_100 = make_array_code(100);
    benchmark_compile("N=100  array", &code_100, &engine, 20);

    // N=500 array
    let code_500 = make_array_code(500);
    benchmark_compile("N=500  array", &code_500, &engine, 20);

    // N=1000 array (target: < 5ms)
    let code_1000 = make_array_code(1000);
    benchmark_compile("N=1000 array", &code_1000, &engine, 10);

    // N=2000 array (target: < 10ms)
    let code_2000 = make_array_code(2000);
    benchmark_compile("N=2000 array", &code_2000, &engine, 10);

    println!();
    println!("=== Acceptance Criteria ===");
    println!("  N=1000 codegen < 5ms  (spec target)");
    println!("  N=2000 codegen < 10ms (spec target)");
    println!(
        "  Note: compile time includes parse+IR+codegen, so if compile < target, codegen < target"
    );
}
