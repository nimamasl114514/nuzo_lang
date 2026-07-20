//! 最小复现：隔离栈溢出
use nuzo_compiler::Compiler;
use nuzo_vm::VM;
use std::sync::{Arc, Mutex};

fn test(source: &str, _name: &str) -> Result<Vec<String>, String> {
    eprintln!("  [1] 开始编译...");
    let chunk = Compiler::compile(source).map_err(|e| format!("编译: {}", e))?;
    eprintln!("  [2] 编译完成，开始创建VM...");
    let (mut vm, buf): (VM, Arc<Mutex<Vec<String>>>) = VM::new_with_output_capture();
    eprintln!("  [3] VM创建完成，开始执行...");
    vm.run(chunk).map_err(|e| format!("执行: {}", e))?;
    eprintln!("  [4] 执行完成");
    let result = buf.lock().unwrap().clone();
    Ok(result)
}

fn main() {
    let cases = vec![("简单print", r#"println("hello")"#)];

    for (name, src) in &cases {
        eprintln!(">>> 测试: {}", name);
        match test(src, name) {
            Ok(out) => eprintln!("    OK: {:?}", out),
            Err(e) => eprintln!("    FAIL: {}", e),
        }
    }
}
