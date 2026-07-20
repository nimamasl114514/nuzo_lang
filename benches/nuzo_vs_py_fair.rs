/// Nuzo vs Python — 公平对比（Nuzo 用 Engine API，无子进程开销）
/// Python 数据通过 stdin 传入，Nuzo 用内嵌 Engine
use nuzo_run::Engine;
use std::time::Instant;

fn main() {
    let engine = Engine::quick().expect("Engine 创建失败");

    println!("=== Nuzo vs Python 公平对比 ===");
    println!("Nuzo: Engine API (无子进程开销)");
    println!("Python: 3.12.13 CPython (in-process exec)");
    println!("预热: 3 轮, 采样: 15 轮\n");

    let benchmarks: Vec<(&str, &str)> = vec![
        ("A: 算术循环 100k", "sum = 0\nfor i in 0..100000 {\n  sum = sum + i\n}"),
        ("B: 字符串拼接 5k", "s = \"\"\nfor i in 0..5000 {\n  s = s + \"x\" + i\n}"),
        (
            "C: 嵌套循环 316x316",
            "sum = 0\nfor i in 0..316 {\n  for j in 0..316 {\n    sum = sum + i * j\n  }\n}",
        ),
        (
            "D: 函数调用 50k",
            "fn add(a, b) {\n  return a + b\n}\nsum = 0\nfor i in 0..50000 {\n  sum = add(sum, i)\n}",
        ),
        ("E: 累加 500k", "sum = 0\nfor i in 0..500000 {\n  sum = sum + 1\n}"),
        (
            "F: 条件分支 100k",
            "sum = 0\nfor i in 0..100000 {\n  if i % 2 == 0 {\n    sum = sum + 1\n  } else {\n    sum = sum - 1\n  }\n}",
        ),
        (
            "G: 局部变量 100k",
            "a = 1\nb = 2\nc = 3\nd = 4\ne = 5\nsum = 0\nfor i in 0..100000 {\n  sum = a + b + c + d + e\n  a = a + 1\n}",
        ),
    ];

    let warmup = 3;
    let samples = 15;

    // === Nuzo 测试 ===
    println!("--- Nuzo 测试 ---");
    let mut nuzo_results: Vec<(&str, f64)> = Vec::new();

    for (name, script) in &benchmarks {
        for _ in 0..warmup {
            let _ = engine.run(script);
        }
        let mut times: Vec<f64> = Vec::new();
        for _ in 0..samples {
            let start = Instant::now();
            let r = engine.run(script);
            std::hint::black_box(&r);
            times.push(start.elapsed().as_nanos() as f64 / 1000.0);
        }
        times.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let median = times[times.len() / 2];
        println!("  {:<24} {:>10.1} us", name, median);
        nuzo_results.push((name, median));
    }

    // === Python 数据（从上一次测试结果手动录入） ===
    println!("\n--- Python 3.12 数据 ---");
    let python_results: Vec<(&str, f64)> = vec![
        ("A: 算术循环 100k", 7312.0),
        ("B: 字符串拼接 5k", 3447.0),
        ("C: 嵌套循环 316x316", 11575.0),
        ("D: 函数调用 50k", 6976.0),
        ("E: 累加 500k", 44596.0),
        ("F: 条件分支 100k", 10378.0),
        ("G: 局部变量 100k", 18119.0),
    ];
    for (name, us) in &python_results {
        println!("  {:<24} {:>10.1} us", name, us);
    }

    // === 汇总 ===
    println!("\n{}", "=".repeat(80));
    println!(
        "{:<26} {:>12} {:>12} {:>10} {:>8}",
        "场景", "Python(us)", "Nuzo(us)", "Nuzo/Py", "胜者"
    );
    println!("{}", "-".repeat(80));

    for (i, (name, nuzo_us)) in nuzo_results.iter().enumerate() {
        let py_us = python_results[i].1;
        let ratio = nuzo_us / py_us;
        let winner = if *nuzo_us < py_us { "Nuzo" } else { "Python" };
        println!("{:<26} {:>12.0} {:>12.0} {:>9.2}x {:>8}", name, py_us, nuzo_us, ratio, winner);
    }
    println!("{}", "=".repeat(80));
    println!("\n* Nuzo/Py < 1.0 = Nuzo 更快, > 1.0 = Python 更快");
    println!("* 两端均无子进程开销，纯执行时间");
}
