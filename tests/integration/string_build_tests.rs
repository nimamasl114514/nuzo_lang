//! StringBuild 编译期拼接树分析端到端测试
//!
//! 验证连续 `+` 链（≥3 操作数且含字符串字面量）被编译为 `StringBuild` 指令，
//! 且运行时结果正确。

use nuzo_bytecode::{Chunk, Opcode};
use nuzo_compiler::Compiler;
use nuzo_values::ValueExt;
use nuzo_vm::VM;

/// 递归统计指定 Opcode 在顶层 Chunk 及其所有嵌套 FunctionPrototype 中的出现次数
fn count_opcode_recursive(chunk: &Chunk, opcode: Opcode) -> usize {
    let mut count = count_opcode(chunk, opcode);
    for value in chunk.constants() {
        if let Some(obj) = value.as_heap_object_opt()
            && let nuzo_values::heap::HeapObject::Closure { prototype, .. } = obj.as_ref()
        {
            let sub_chunk = Chunk::from_arcs(
                std::sync::Arc::clone(&prototype.chunk),
                std::sync::Arc::clone(&prototype.constants),
                std::sync::Arc::clone(&prototype.lines),
                std::sync::Arc::clone(&prototype.debug_info),
                prototype.locals_count,
                prototype.spill_slot_count,
            );
            count += count_opcode_recursive(&sub_chunk, opcode);
        }
    }
    count
}

/// 编译源码并返回 Chunk
fn compile(source: &str) -> Chunk {
    Compiler::compile(source).expect("编译失败")
}

/// 统计指定 Opcode 在顶层 Chunk 中的出现次数
fn count_opcode(chunk: &Chunk, opcode: Opcode) -> usize {
    let code = chunk.code();
    let mut count = 0;
    let mut ip = 0;
    while ip < code.len() {
        let op = match Opcode::decode_opcode(code[ip]) {
            Some(op) => op,
            None => break,
        };
        if op == opcode {
            count += 1;
        }
        let size = op.instruction_size();
        if size == 0 || ip + size > code.len() {
            break;
        }
        ip += size;
    }
    count
}

/// 编译并在 VM 中运行，返回结果值的字符串表示
fn compile_and_run(source: &str) -> Result<String, String> {
    let chunk = compile(source);
    let mut vm = VM::new();
    match vm.run(chunk) {
        Ok(result) => Ok(result.concat_repr()),
        Err(e) => Err(format!("VM error: {}", e)),
    }
}

// ============================================================================
// 指令生成验证：StringBuild 指令应被生成
// ============================================================================

#[test]
fn test_string_chain_generates_stringbuild() {
    let chunk = compile(r#""a" + "b" + "c""#);
    let sb_count = count_opcode(&chunk, Opcode::StringBuild);
    assert_eq!(sb_count, 1, "3 个字符串字面量的 + 链应生成 1 条 StringBuild 指令");
}

#[test]
fn test_four_string_chain_generates_stringbuild() {
    let chunk = compile(r#"let prefix = "Hello, "; prefix + "World" + "!" + "!!""#);
    let sb_count = count_opcode(&chunk, Opcode::StringBuild);
    assert_eq!(sb_count, 1, "4 个操作数（含变量）的字符串 + 链应生成 1 条 StringBuild");
}

#[test]
fn test_long_string_chain_generates_single_stringbuild() {
    let chunk = compile(r#""a" + "b" + "c" + "d" + "e" + "f" + "g""#);
    let sb_count = count_opcode(&chunk, Opcode::StringBuild);
    assert_eq!(sb_count, 1, "7 个操作数的 + 链应生成 1 条 StringBuild");

    let add_count = count_opcode(&chunk, Opcode::Add);
    assert_eq!(add_count, 0, "字符串 + 链不应生成 Add 指令");
}

// ============================================================================
// 指令不生成验证：纯数字 + 链不应生成 StringBuild
// ============================================================================

#[test]
fn test_pure_numeric_chain_no_stringbuild() {
    // 使用变量避免常量折叠将 1+2+3+4 直接算为 10
    let chunk = compile("fn f(a, b, c, d) { return a + b + c + d }");
    let sb_count = count_opcode(&chunk, Opcode::StringBuild);
    assert_eq!(sb_count, 0, "纯数字 + 链不应生成 StringBuild");

    // 常量折叠会将纯字面量算完，所以用变量
    let add_count = count_opcode_recursive(&chunk, Opcode::Add);
    assert!(add_count >= 1, "纯数字 + 链应生成 Add 指令");
}

#[test]
fn test_two_operand_no_stringbuild() {
    let chunk = compile(r#""a" + "b""#);
    let sb_count = count_opcode(&chunk, Opcode::StringBuild);
    assert_eq!(sb_count, 0, "2 个操作数的 + 不应生成 StringBuild（阈值 >= 3）");
}

// ============================================================================
// 运行时正确性验证
// ============================================================================

#[test]
fn test_string_chain_result_correct() {
    let result = compile_and_run(r#""Hello" + ", " + "World" + "!" "#);
    assert_eq!(result.unwrap(), "Hello, World!");
}

#[test]
fn test_mixed_type_string_chain_correct() {
    let result = compile_and_run(r#""Value: " + 42 + " items""#);
    assert_eq!(result.unwrap(), "Value: 42 items");
}

#[test]
fn test_string_chain_with_variables_correct() {
    let result = compile_and_run(r#"name = "Alice"; "Hello, " + name + "!" + " Welcome""#);
    assert_eq!(result.unwrap(), "Hello, Alice! Welcome");
}

#[test]
fn test_numeric_addition_still_works() {
    let result = compile_and_run("1 + 2 + 3 + 4");
    assert_eq!(result.unwrap(), "10");
}

#[test]
fn test_single_string_no_stringbuild() {
    let result = compile_and_run(r#""just a string""#);
    assert_eq!(result.unwrap(), "just a string");
}
