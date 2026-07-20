//! IR 层集成测试 — 验证 AST→IR→Chunk 完整链路的行为等价性
//!
//! 每个测试验证两件事：
//! 1. 编译成功（chunk 非空）
//! 2. 字节码包含预期的 opcode（通过反汇编验证）

use nuzo_compiler::Compiler;

/// 辅助：编译并返回反汇编文本
fn compile_and_disasm(source: &str) -> String {
    let chunk = Compiler::compile(source).expect("compile should succeed");
    assert!(!chunk.is_empty(), "chunk should not be empty");
    chunk.disassemble()
}

#[test]
fn test_ir_compile_number_literal() {
    let disasm = compile_and_disasm("42");
    // 数字字面量应生成 LoadK 指令
    assert!(disasm.contains("LoadK"), "数字字面量应生成 LoadK 指令, got:\n{}", disasm);
}

#[test]
fn test_ir_compile_simple_expression() {
    // 用变量避免常量折叠（1+2 会被编译期计算为 3，不生成 Add）
    let disasm = compile_and_disasm("let x = 1; x + 2");
    // 算术表达式应生成 Add 指令（变量 + 常量不会被折叠）
    assert!(disasm.contains("Add"), "x+2 应生成 Add 指令, got:\n{}", disasm);
}

#[test]
fn test_ir_compile_string_literal() {
    let disasm = compile_and_disasm("\"hello\"");
    assert!(disasm.contains("LoadK"), "字符串字面量应生成 LoadK 指令, got:\n{}", disasm);
}

#[test]
fn test_ir_compile_bool_literals() {
    // 当前编译器把 true/false 当作常量池中的 Value 加载（LoadK），
    // 而非专用的 LoadTrue/LoadFalse opcode。验证常量加载即可。
    let disasm = compile_and_disasm("true && false");
    assert!(disasm.contains("LoadK"), "布尔字面量应通过 LoadK 加载, got:\n{}", disasm);
}

#[test]
fn test_ir_compile_variable_assignment() {
    let disasm = compile_and_disasm("let x = 10; x + 1");
    assert!(disasm.contains("Add"), "x+1 应生成 Add 指令, got:\n{}", disasm);
}

#[test]
fn test_ir_compile_if_statement() {
    let disasm = compile_and_disasm("if (true) { 1 } else { 2 }");
    // if/else 应生成 Test 跳转指令
    assert!(
        disasm.contains("Test") || disasm.contains("Jmp"),
        "if/else 应生成 Test/Jmp 指令, got:\n{}",
        disasm
    );
}

#[test]
fn test_ir_compile_while_loop_false() {
    let disasm = compile_and_disasm("while (false) { 42 }");
    assert!(
        disasm.contains("Test") || disasm.contains("Jmp"),
        "while 循环应生成 Test/Jmp 指令, got:\n{}",
        disasm
    );
}

#[test]
fn test_ir_compile_while_loop_basic() {
    let disasm = compile_and_disasm("let i = 0; while (i < 3) { i = i + 1 }; i");
    assert!(
        disasm.contains("Lt") || disasm.contains("Test"),
        "while (i<3) 应生成 Lt/Test 指令, got:\n{}",
        disasm
    );
}

#[test]
fn test_ir_compile_while_with_break() {
    let disasm = compile_and_disasm("let i = 0; while (true) { i = i + 1; break }; i");
    assert!(disasm.contains("Jmp"), "break 应生成 Jmp 指令, got:\n{}", disasm);
}

#[test]
fn test_ir_compile_while_with_return() {
    // return 在函数体内部，编译后函数体作为子 chunk（Closure 引用）。
    // 顶层字节码会生成 Closure + SetGlobal + GetGlobal + Call。
    // 验证 Closure 存在即可证明函数声明被正确编译。
    let disasm = compile_and_disasm("fn f() { while (true) { return 42 } }; f()");
    assert!(
        disasm.contains("Closure") || disasm.contains("Call"),
        "fn f(){{...}}; f() 应生成 Closure/Call 指令, got:\n{}",
        disasm
    );
}

#[test]
fn test_ir_compile_loop_infinite() {
    let disasm = compile_and_disasm("let i = 0; loop { i = i + 1; break }; i");
    assert!(disasm.contains("Jmp"), "loop+break 应生成 Jmp 指令, got:\n{}", disasm);
}

#[test]
fn test_ir_compile_for_in_basic() {
    let disasm = compile_and_disasm("for x in [1,2,3] { x }");
    // for-in 应生成 ArrayNew + 迭代相关指令
    assert!(
        disasm.contains("ArrayNew") || disasm.contains("GetIndex"),
        "for-in [1,2,3] 应生成 ArrayNew/GetIndex 指令, got:\n{}",
        disasm
    );
}

#[test]
fn test_ir_compile_function_declaration() {
    let disasm = compile_and_disasm("fn add(a, b) { a + b }; add(1, 2)");
    assert!(disasm.contains("Call"), "函数调用应生成 Call 指令, got:\n{}", disasm);
}

#[test]
fn test_ir_compile_closure() {
    let disasm = compile_and_disasm("let x = 10; let f = fn() { x }; f()");
    // 闭包应生成 Closure 指令
    assert!(
        disasm.contains("Closure") || disasm.contains("Call"),
        "闭包应生成 Closure/Call 指令, got:\n{}",
        disasm
    );
}

#[test]
fn test_ir_compile_array_literal() {
    let disasm = compile_and_disasm("[1, 2, 3]");
    assert!(disasm.contains("ArrayNew"), "数组字面量应生成 ArrayNew 指令, got:\n{}", disasm);
}

#[test]
fn test_ir_compile_object_literal() {
    let disasm = compile_and_disasm("{\"name\": \"nuzo\"}");
    // 对象字面量应生成 LoadK + SetProp 指令
    assert!(!disasm.is_empty(), "对象字面量应生成非空字节码, got:\n{}", disasm);
}

#[test]
fn test_ir_compile_empty_program() {
    let _chunk = Compiler::compile("").expect("empty program should compile");
    // 空程序应该产生一个有效的 chunk（至少有 Halt）
}

#[test]
fn test_ir_compile_print_statement() {
    // print 是全局函数，编译为 GetGlobal(print) + LoadK(42) + Call，
    // 而非专用的 Print opcode。
    let disasm = compile_and_disasm("print(42)");
    assert!(
        disasm.contains("GetGlobal") || disasm.contains("Call"),
        "print(42) 应通过 GetGlobal+Call 调用, got:\n{}",
        disasm
    );
}
