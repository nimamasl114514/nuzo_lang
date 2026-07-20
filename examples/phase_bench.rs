//! 分阶段性能剖析：精确测量 compile / execute 各占多少时间。
//!
//! 运行: cargo run --release --example phase_bench
//!
//! 对比 perf_test.py 的总时间（含进程启动），本工具在进程内测量，
//! 排除启动开销，分别给出 compile-only 与 execute-only 的耗时。

use nuzo::{Engine, OutputSink};
use std::time::Instant;

fn make_array_code(n: usize) -> String {
    let nums: Vec<String> = (0..n).map(|i| i.to_string()).collect();
    format!("len([{}])", nums.join(","))
}

fn bench_phases(label: &str, source: &str, runs: usize) {
    let engine = Engine::builder().with_default_config().build().expect("engine build");

    // --- compile-only ---
    // warmup
    let _ = engine.compile(source);
    let mut compile_times = Vec::with_capacity(runs);
    for _ in 0..runs {
        let t = Instant::now();
        let _ = engine.compile(source);
        compile_times.push(t.elapsed().as_nanos() as f64 / 1_000_000.0); // ms
    }
    let compile_mean = compile_times.iter().sum::<f64>() / runs as f64;
    let compile_min = compile_times.iter().cloned().fold(f64::INFINITY, f64::min);

    // --- execute-only (compile once, then time execution) ---
    let chunk = engine.compile(source).expect("compile");
    // warmup
    {
        let mut s = engine.new_session_with(OutputSink::Null);
        let _ = s.execute(chunk.clone());
    }
    let mut exec_times = Vec::with_capacity(runs);
    for _ in 0..runs {
        let mut s = engine.new_session_with(OutputSink::Null);
        let t = Instant::now();
        let _ = s.execute(chunk.clone());
        exec_times.push(t.elapsed().as_nanos() as f64 / 1_000_000.0); // ms
    }
    let exec_mean = exec_times.iter().sum::<f64>() / runs as f64;
    let exec_min = exec_times.iter().cloned().fold(f64::INFINITY, f64::min);

    // --- end-to-end (compile + execute) ---
    let mut e2e_times = Vec::with_capacity(runs);
    for _ in 0..runs {
        let mut s = engine.new_session_with(OutputSink::Null);
        let t = Instant::now();
        let _ = s.run(source);
        e2e_times.push(t.elapsed().as_nanos() as f64 / 1_000_000.0); // ms
    }
    let e2e_mean = e2e_times.iter().sum::<f64>() / runs as f64;
    let e2e_min = e2e_times.iter().cloned().fold(f64::INFINITY, f64::min);

    let compile_pct = compile_mean / e2e_mean * 100.0;
    let exec_pct = exec_mean / e2e_mean * 100.0;

    println!("┌─ {} ─", label);
    println!("│  源码长度: {} 字符", source.len());
    println!(
        "│  compile-only:  mean={:>7.3} ms  min={:>7.3} ms  ({:>5.1}% of e2e)",
        compile_mean, compile_min, compile_pct
    );
    println!(
        "│  execute-only:  mean={:>7.3} ms  min={:>7.3} ms  ({:>5.1}% of e2e)",
        exec_mean, exec_min, exec_pct
    );
    println!("│  end-to-end:    mean={:>7.3} ms  min={:>7.3} ms", e2e_mean, e2e_min);
    println!("│  runs: {}", runs);
    println!("└─");
    println!();
}

fn main() {
    println!("Nuzo Lang 分阶段性能剖析 (release, in-process, 排除进程启动)\n");

    let code_1000 = make_array_code(1000);
    let code_2000 = make_array_code(2000);

    bench_phases("N=1000 数组 len()", &code_1000, 20);
    bench_phases("N=2000 数组 len()", &code_2000, 20);

    println!("验收目标 (codegen): N=1000 < 5ms, N=2000 < 10ms");
    println!("注: compile-only ≈ lex+parse+IR+codegen (codegen 是其子集)");
}
