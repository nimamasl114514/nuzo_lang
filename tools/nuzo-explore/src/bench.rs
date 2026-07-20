//! Nuzo 性能基准测试 — 测量编译和执行时间
//! 与 Rust 原生实现对比，发现瓶颈

#![allow(dead_code, unused_variables, unused_assignments)]

use nuzo_compiler::Compiler;
use nuzo_vm::VM;
use std::sync::{Arc, Mutex};
use std::time::Instant;

pub struct BenchResult {
    pub name: &'static str,
    pub compile_ms: f64,
    pub exec_ms: f64,
    pub output: String,
}

fn bench(source: &str) -> Result<BenchResult, String> {
    // 编译计时
    let t0 = Instant::now();
    let chunk = Compiler::compile(source).map_err(|e| format!("编译: {}", e))?;
    let compile_ms = t0.elapsed().as_secs_f64() * 1000.0;

    // 执行计时
    let (mut vm, output_buf): (VM, Arc<Mutex<Vec<String>>>) = VM::new_with_output_capture();
    let t0 = Instant::now();
    vm.run(chunk).map_err(|e| format!("执行: {}", e))?;
    let exec_ms = t0.elapsed().as_secs_f64() * 1000.0;

    let output = output_buf.lock().unwrap().join("\n");
    Ok(BenchResult { name: "", compile_ms, exec_ms, output })
}

pub fn run_all() {
    println!("\n{:=^70}", " 性能基准测试 ");
    println!();

    let mut results: Vec<BenchResult> = Vec::new();

    // Helper macro for push
    macro_rules! bench_push {
        ($name:expr, $source:expr) => {
            match bench($source) {
                Ok(mut r) => {
                    r.name = $name;
                    results.push(r);
                }
                Err(e) => eprintln!("  [SKIP] {}: {}", $name, e),
            }
        };
    }

    // 1. 斐波那契递归 (fib 30)
    bench_push!(
        "递归fib(30)",
        r#"
        fn fib(n) {
            if n <= 1 { return n }
            return fib(n - 1) + fib(n - 2)
        }
        println(fib(30))
    "#
    );

    // 2. 尾递归计数 (10万次)
    bench_push!(
        "尾递归count(10万)",
        r#"
        fn count(n, acc) {
            if n <= 0 { return acc }
            return count(n - 1, acc + 1)
        }
        println(count(100000, 0))
    "#
    );

    // 3. while循环累加 (100万次)
    bench_push!(
        "while累加(100万)",
        r#"
        sum = 0
        i = 0
        while i < 1000000 {
            sum = sum + i
            i = i + 1
        }
        println(sum)
    "#
    );

    // 4. 数组push (1万元素)
    bench_push!(
        "数组push(1万)",
        r#"
        arr = []
        i = 0
        while i < 10000 {
            push(arr, i)
            i = i + 1
        }
        println(len(arr))
    "#
    );

    // 5. 字典插入 (1000键) — 注意: Nuzo 字典键必须是字符串
    bench_push!(
        "字典插入(1千)",
        r#"
        d = {}
        i = 0
        while i < 1000 {
            d["k" + i] = i * i
            i = i + 1
        }
        println(d["k999"])
    "#
    );

    // 6. 字符串拼接 (1000次)
    bench_push!(
        "字符串拼接(1千次)",
        r#"
        s = ""
        i = 0
        while i < 1000 {
            s = s + "x"
            i = i + 1
        }
        println(len(s))
    "#
    );

    // 7. 嵌套循环 (100x100)
    bench_push!(
        "嵌套循环(100x100)",
        r#"
        sum = 0
        i = 0
        while i < 100 {
            j = 0
            while j < 100 {
                sum = sum + 1
                j = j + 1
            }
            i = i + 1
        }
        println(sum)
    "#
    );

    // 8. 闭包调用 (1000次)
    bench_push!(
        "闭包调用(1千次)",
        r#"
        fn make_fn(x) {
            return fn(y) { x + y }
        }
        f = make_fn(1)
        i = 0
        sum = 0
        while i < 1000 {
            sum = sum + f(i)
            i = i + 1
        }
        println(sum)
    "#
    );

    // 9. 大量表达式 (长算术链)
    bench_push!(
        "长算术链",
        r#"
        x = 1 + 2 * 3 - 4 / 2 + 5 * 6 - 7 + 8 * 9 / 3 - 10 + 11 * 12 / 4
        println(x)
    "#
    );

    // 10. 递归深度 (非尾递归 20层)
    bench_push!(
        "非尾递归深度(20)",
        r#"
        fn deep(n) {
            if n <= 0 { return 0 }
            return 1 + deep(n - 1)
        }
        println(deep(20))
    "#
    );

    // 打印结果
    println!("{:30} | {:>10} | {:>10} | {:>10}", "测试", "编译(ms)", "执行(ms)", "总计(ms)");
    println!("{:-<70}", "");
    let mut total_compile = 0.0;
    let mut total_exec = 0.0;
    for r in &results {
        println!(
            "{:30} | {:10.3} | {:10.3} | {:10.3}",
            r.name,
            r.compile_ms,
            r.exec_ms,
            r.compile_ms + r.exec_ms
        );
        total_compile += r.compile_ms;
        total_exec += r.exec_ms;
    }
    println!("{:-<70}", "");
    println!(
        "{:30} | {:10.3} | {:10.3} | {:10.3}",
        "合计",
        total_compile,
        total_exec,
        total_compile + total_exec
    );
    println!();

    // Rust 原生对比
    println!("{:=^70}", " Rust 原生对比 (同一算法) ");
    println!();
    rust_native_bench();
}

fn rust_native_bench() {
    // 与 Nuzo 对应算法的 Rust 原生实现计时

    // fib(30)
    fn fib(n: u64) -> u64 {
        if n <= 1 {
            n
        } else {
            fib(n - 1) + fib(n - 2)
        }
    }
    let t0 = Instant::now();
    let _ = fib(30);
    let rust_fib = t0.elapsed().as_secs_f64() * 1000.0;

    // while 累加 100万
    let t0 = Instant::now();
    let mut sum: u64 = 0;
    let mut i: u64 = 0;
    while i < 1_000_000 {
        sum += i;
        i += 1;
    }
    let rust_while = t0.elapsed().as_secs_f64() * 1000.0;

    // 数组 push 1万
    let t0 = Instant::now();
    let mut arr = Vec::new();
    for i in 0..10000u64 {
        arr.push(i);
    }
    let rust_arr = t0.elapsed().as_secs_f64() * 1000.0;

    // 字符串拼接 1000次
    let t0 = Instant::now();
    let mut s = String::new();
    for _ in 0..1000 {
        s.push('x');
    }
    let rust_str = t0.elapsed().as_secs_f64() * 1000.0;

    // 嵌套循环 100x100
    let t0 = Instant::now();
    let mut sum: u64 = 0;
    for _ in 0..100 {
        for _ in 0..100 {
            sum += 1;
        }
    }
    let rust_nested = t0.elapsed().as_secs_f64() * 1000.0;

    println!("{:30} | {:>12} | {:>12}", "测试", "Rust(ms)", "Nuzo比Rust慢");
    println!("{:-<60}", "");
    println!("{:30} | {:12.4} | {:>10.0}x", "递归fib(30)", rust_fib, 0.0);
    println!("{:30} | {:12.4} | {:>10.0}x", "while累加(100万)", rust_while, 0.0);
    println!("{:30} | {:12.4} | {:>10.0}x", "数组push(1万)", rust_arr, 0.0);
    println!("{:30} | {:12.4} | {:>10.0}x", "字符串拼接(1千)", rust_str, 0.0);
    println!("{:30} | {:12.4} | {:>10.0}x", "嵌套循环(100x100)", rust_nested, 0.0);
    println!();
}
