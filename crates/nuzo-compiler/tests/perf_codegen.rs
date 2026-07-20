use nuzo_compiler::Compiler;
use std::time::Instant;

fn make_array_code(n: usize) -> String {
    let elements: Vec<String> = (0..n).map(|i| i.to_string()).collect();
    format!("len([{}])", elements.join(","))
}

fn bench_codegen(name: &str, n: usize, threshold_ms: f64) {
    let code = make_array_code(n);

    let mut times = Vec::new();
    for _ in 0..10 {
        let start = Instant::now();
        let result = Compiler::compile(&code);
        let elapsed = start.elapsed();
        assert!(result.is_ok(), "compile failed: {:?}", result.err());
        times.push(elapsed.as_secs_f64() * 1000.0);
    }

    times.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let median = times[times.len() / 2];
    let min = times[0];
    let max = times[times.len() - 1];

    let status = if median < threshold_ms { "PASS" } else { "FAIL" };
    println!(
        "{}: N={} median={:.3}ms min={:.3}ms max={:.3}ms threshold={:.1}ms [{}]",
        name, n, median, min, max, threshold_ms, status
    );

    assert!(
        median < threshold_ms,
        "{} N={} codegen median {:.3}ms exceeds threshold {:.1}ms",
        name,
        n,
        median,
        threshold_ms
    );
}

// 性能测试设计为 release 模式运行（debug 模式未优化，性能自然达不到 release 阈值）。
// debug_assertions 在 debug 模式为 true，release 模式为 false。
// 因此 debug 模式跳过这两个测试，release 模式正常运行。
#[test]
#[cfg_attr(debug_assertions, ignore = "perf test requires --release")]
fn test_codegen_perf_n1000() {
    bench_codegen("codegen_perf", 1000, 5.0);
}

#[test]
#[cfg_attr(debug_assertions, ignore = "perf test requires --release")]
fn test_codegen_perf_n2000() {
    bench_codegen("codegen_perf", 2000, 10.0);
}
