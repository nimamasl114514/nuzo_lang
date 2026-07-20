/// VM 分支预测提示 — 性能基准测试
///
/// 通过 nuzo_run::Engine 执行 Nuzo 脚本，测量 VM 主循环在不同工作负载下的性能。
use std::time::Instant;

fn main() {
    use nuzo_run::Engine;

    println!("=== VM 分支预测提示 — 性能基准 ===\n");
    println!("初始化 Engine...");

    let engine = Engine::quick().expect("Engine 创建失败");

    let config = BenchConfig { warmup: 3, samples: 20 };

    // VM-A: 纯算术循环 100k
    let mut results: Vec<BenchResult> = vec![run_bench(
        &engine,
        &config,
        "VM-A",
        "纯算术循环(100k+加法)",
        r#"
            sum = 0
            for i in 0..100000 {
                sum = sum + i
            }
        "#,
    )];

    // VM-B: 字符串拼接 5000 (触发 StringBuild)
    results.push(run_bench(
        &engine,
        &config,
        "VM-B",
        "字符串拼接(5000次)",
        r#"
            s = ""
            for i in 0..5000 {
                s = s + "x" + i
            }
        "#,
    ));

    // VM-C: 函数调用 50000
    results.push(run_bench(
        &engine,
        &config,
        "VM-D",
        "函数调用(5万call/return)",
        r#"
            fn add(a, b) {
                return a + b
            }
            sum = 0
            for i in 0..50000 {
                sum = add(sum, i)
            }
        "#,
    ));

    // VM-E: 简单累加 500k (极致热点)
    results.push(run_bench(
        &engine,
        &config,
        "VM-E",
        "简单累加(50万次)",
        r#"
            sum = 0
            for i in 0..500000 {
                sum = sum + 1
            }
        "#,
    ));

    // VM-F: 嵌套循环 (更多跳转分支)
    results.push(run_bench(
        &engine,
        &config,
        "VM-F",
        "嵌套循环(316x316)",
        r#"
            sum = 0
            for i in 0..316 {
                for j in 0..316 {
                    sum = sum + i * j
                }
            }
        "#,
    ));

    // VM-G: 条件分支密集 (测分支预测)
    results.push(run_bench(
        &engine,
        &config,
        "VM-G",
        "条件分支(10万次 if/else)",
        r#"
            sum = 0
            for i in 0..100000 {
                if i % 2 == 0 {
                    sum = sum + 1
                } else {
                    sum = sum - 1
                }
            }
        "#,
    ));

    // VM-H: 大量局部变量读写 (测寄存器文件)
    results.push(run_bench(
        &engine,
        &config,
        "VM-H",
        "局部变量读写(10万次)",
        r#"
            a = 1
            b = 2
            c = 3
            d = 4
            e = 5
            sum = 0
            for i in 0..100000 {
                sum = a + b + c + d + e
                a = a + 1
            }
        "#,
    ));

    // 打印结果
    println!(
        "\n{:<8} {:<34} {:>10} {:>10} {:>10} {:>10} {:>10}",
        "ID", "场景", "Mean(us)", "Median", "P95", "P99", "Min"
    );
    println!("{}", "-".repeat(94));

    for r in &results {
        let name_display = truncate(&r.name, 32);
        println!(
            "{:<8} {:<34} {:>10.1} {:>10.1} {:>10.1} {:>10.1} {:>10.1}",
            r.id, name_display, r.mean_us, r.median_us, r.p95_us, r.p99_us, r.min_us
        );
    }

    let total_rounds = config.warmup + config.samples;
    println!("\n* 单位: 微秒(us), 越小越好");
    println!("* 编译: release (optimized)");
    println!("* 采样: {} 轮 ({} 预热 + {} 采样)", total_rounds, config.warmup, config.samples);
}

fn run_bench(
    engine: &nuzo_run::Engine,
    config: &BenchConfig,
    id: &str,
    name: &str,
    script: &str,
) -> BenchResult {
    println!("  [{}] 预热 {} 轮...", id, config.warmup);
    for _ in 0..config.warmup {
        let _ = engine.run(script);
    }

    println!("  [{}] 采样 {} 轮...", id, config.samples);
    let mut samples_ns: Vec<f64> = Vec::with_capacity(config.samples);

    for _ in 0..config.samples {
        let start = Instant::now();
        let result = engine.run(script);
        let elapsed = start.elapsed().as_nanos() as f64;

        std::hint::black_box(&result);

        if let Err(e) = result {
            println!("  [{}] 错误: {:?}", id, e);
        }
        samples_ns.push(elapsed);
    }

    samples_ns.sort_by(|a, b| a.partial_cmp(b).unwrap());

    let n = samples_ns.len() as f64;
    let mean = samples_ns.iter().sum::<f64>() / n;
    let median = samples_ns[n as usize / 2];
    let p95_idx = ((n * 0.95) as usize).min(samples_ns.len() - 1);
    let p99_idx = ((n * 0.99) as usize).min(samples_ns.len() - 1);
    let p95 = samples_ns[p95_idx];
    let p99 = samples_ns[p99_idx];

    println!("  [{}] mean={:.1}us median={:.1}us", id, mean / 1000.0, median / 1000.0);

    BenchResult {
        id: id.to_string(),
        name: name.to_string(),
        mean_us: mean / 1000.0,
        median_us: median / 1000.0,
        p95_us: p95 / 1000.0,
        p99_us: p99 / 1000.0,
        min_us: samples_ns[0] / 1000.0,
    }
}

struct BenchConfig {
    warmup: usize,
    samples: usize,
}

struct BenchResult {
    id: String,
    name: String,
    mean_us: f64,
    median_us: f64,
    p95_us: f64,
    p99_us: f64,
    min_us: f64,
}

fn truncate(s: &str, max: usize) -> String {
    let cnt = s.chars().count();
    if cnt > max { s.chars().take(max).collect() } else { s.to_string() }
}
