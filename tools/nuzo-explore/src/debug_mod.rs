use nuzo_compiler::Compiler;
fn main() {
    let tests = [
        ("2 ** 3 ** 2", "512"), // 右结合: 2^(3^2) = 2^9 = 512
        ("2 ** 3", "8"),
        ("2 ** 10", "1024"),
        ("println(10 % 3)", "1"),
        ("println(10 mod 3)", "1"),
        ("println(-10 % 3)", "-1"),
        ("typeof(42)", "number"),
        ("typeof(\"hello\")", "string"),
        ("typeof(true)", "bool"),
        ("typeof(nil)", "nil"),
        ("typeof([1,2])", "array"),
        ("typeof({a:1})", "object"),
    ];
    for (src, expected) in &tests {
        match Compiler::compile(src) {
            Ok(c) => println!("OK [{}] (expect {}): locals={}", src, expected, c.locals_count),
            Err(e) => println!("ERR [{}]: {}", src, e),
        }
    }
}
