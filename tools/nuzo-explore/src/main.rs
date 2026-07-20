mod bench;
mod report;
mod tests;

fn main() {
    // Windows 默认 1MB 栈不够，需要 8MB
    let child = std::thread::Builder::new().stack_size(8 * 1024 * 1024).spawn(run).unwrap();
    child.join().unwrap();
}

fn run() {
    println!("=== Nuzo 语言探索项目 ===\n");

    // 运行所有探索性测试
    tests::run_all();

    // 性能基准暂时禁用（fib(30) 耗时 ~9s）
    // bench::run_all();
    // report::generate();
}
