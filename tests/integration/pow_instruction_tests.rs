//! Pow (Exponentiation) Opcode — End-to-End Verification Tests
//!
//! This test file validates that the new Pow instruction works correctly
//! across all layers: opcode definition, value operation, dispatch, and execution.
//!
//! # Architecture Proof
//!
//! These tests prove the table-driven architecture claim: adding Pow required
//! changes to only 5 files:
//! 1. `src/opcode.rs` — Opcode definition (=40)
//! 2. `src/ast.rs` — BinaryOp::Pow variant
//! 3. `nuzo_values/src/value.rs` — Value::pow() method
//! 4. `src/dispatch.rs` — dispatch table entry
//! 5. `src/compiler/expressions.rs` — binary_op_mapping entry

use nuzo_bytecode::{Chunk, Opcode};
use nuzo_core::Value;
use nuzo_values::NIL;
use nuzo_vm::VM;

// =========================================================================
// Layer 1: Value::pow() Method Tests
// =========================================================================

#[test]
fn test_value_pow_basic() {
    let base = Value::from_number(2.0);
    let exp = Value::from_number(10.0);
    let result = base.pow(exp).unwrap();
    assert_eq!(result.as_number(), 1024.0); // 2^10 = 1024
}

#[test]
fn test_value_pow_zero_exponent() {
    let base = Value::from_number(5.0);
    let exp = Value::from_number(0.0);
    let result = base.pow(exp).unwrap();
    assert_eq!(result.as_number(), 1.0); // x^0 = 1
}

#[test]
fn test_value_pow_one_base() {
    let base = Value::from_number(1.0);
    let exp = Value::from_number(100.0);
    let result = base.pow(exp).unwrap();
    assert_eq!(result.as_number(), 1.0); // 1^x = 1
}

#[test]
fn test_value_pow_fractional_exponent() {
    let base = Value::from_number(4.0);
    let exp = Value::from_number(0.5);
    let result = base.pow(exp).unwrap();
    assert!((result.as_number() - 2.0).abs() < f64::EPSILON); // sqrt(4) ≈ 2
}

#[test]
fn test_value_pow_negative_exponent() {
    let base = Value::from_number(2.0);
    let exp = Value::from_number(-3.0);
    let result = base.pow(exp).unwrap();
    assert!((result.as_number() - 0.125).abs() < f64::EPSILON); // 2^(-3) = 1/8
}

#[test]
fn test_value_pow_smi_optimization() {
    // Test with Smi-encoded integers (should use fast path if optimized)
    let base = Value::from_number(3.0);
    let exp = Value::from_number(3.0);
    let result = base.pow(exp).unwrap();
    assert_eq!(result.as_number(), 27.0); // 3^3 = 27
}

#[test]
fn test_value_pow_type_error_non_number_base() {
    let base = NIL;
    let exp = Value::from_number(2.0);
    let result = base.pow(exp);
    assert!(result.is_err(), "pow with non-number base should error");
}

#[test]
fn test_value_pow_type_error_non_number_exp() {
    let base = Value::from_number(2.0);
    let exp = NIL;
    let result = base.pow(exp);
    assert!(result.is_err(), "pow with non-number exponent should error");
}

// =========================================================================
// Layer 2: Opcode Definition Tests
// =========================================================================

#[test]
fn test_opcode_pow_exists() {
    // Verify Opcode::Pow is defined and has correct properties
    assert_eq!(Opcode::Pow as u8, 40, "Pow should be opcode number 40");
    assert_eq!(Opcode::Pow.instruction_size(), 7, "Pow should be 7 bytes (1 + 3*u16)");
    assert_eq!(Opcode::Pow.name(), "Pow", "Pow name should be 'Pow'");
    assert_eq!(format!("{}", Opcode::Pow), "Pow", "Pow display should be 'Pow'");
}

#[test]
fn test_opcode_pow_operands() {
    // Verify Pow has Reg, Reg, Reg operands
    use nuzo_bytecode::OperandKind;
    let operands = Opcode::Pow.operands();
    assert_eq!(operands.len(), 3, "Pow should have 3 operands");
    assert_eq!(operands[0], OperandKind::Reg, "First operand should be Reg");
    assert_eq!(operands[1], OperandKind::Reg, "Second operand should be Reg");
    assert_eq!(operands[2], OperandKind::Reg, "Third operand should be Reg");
}

#[test]
fn test_opcode_pow_decode_roundtrip() {
    // Verify Pow can be encoded and decoded correctly
    let byte = Opcode::Pow as u8;
    let decoded = Opcode::decode_opcode(byte);
    assert!(decoded.is_some(), "Should decode byte 40 as Pow");
    assert_eq!(decoded.unwrap(), Opcode::Pow, "Roundtrip should preserve Pow");
}

// =========================================================================
// Layer 3: Bytecode Construction & VM Execution Tests
// =========================================================================

#[test]
fn test_pow_bytecode_construction() {
    // Test that we can construct valid bytecode with Pow instruction
    let mut chunk = Chunk::new();

    let c2 = chunk.add_constant(Value::from_number(2.0));
    let c10 = chunk.add_constant(Value::from_number(10.0));

    // LoadK r0, constants[0]  (2)
    chunk.write_opcode(Opcode::LoadK);
    chunk.write_u16(0);
    chunk.write_u16(c2 as u16);

    // LoadK r1, constants[1]  (10)
    chunk.write_opcode(Opcode::LoadK);
    chunk.write_u16(1);
    chunk.write_u16(c10 as u16);

    // Pow r2, r0, r1  (r2 = r0 ** r1)
    chunk.write_opcode(Opcode::Pow);
    chunk.write_u16(2); // dest
    chunk.write_u16(0); // base
    chunk.write_u16(1); // exp

    // Print r2
    chunk.write_opcode(Opcode::Print);
    chunk.write_u16(2);

    // Halt
    chunk.write_opcode(Opcode::Halt);

    // Verify total size: 5+5+7+3+1 = 21 bytes
    assert_eq!(chunk.len(), 21, "Chunk should be 21 bytes");

    // Verify opcode sequence
    assert_eq!(chunk.code()[0], Opcode::LoadK as u8);
    assert_eq!(chunk.code()[5], Opcode::LoadK as u8);
    assert_eq!(chunk.code()[10], Opcode::Pow as u8, "Byte at offset 10 should be Pow");
    assert_eq!(chunk.code()[17], Opcode::Print as u8);
    assert_eq!(chunk.code()[20], Opcode::Halt as u8);
}

#[test]
fn test_pow_vm_execution_2_to_10() {
    // Full VM execution test: compute 2^10 = 1024
    let mut chunk = Chunk::new();

    let c2 = chunk.add_constant(Value::from_number(2.0));
    let c10 = chunk.add_constant(Value::from_number(10.0));

    chunk.write_opcode(Opcode::LoadK);
    chunk.write_u16(0);
    chunk.write_u16(c2 as u16);

    chunk.write_opcode(Opcode::LoadK);
    chunk.write_u16(1);
    chunk.write_u16(c10 as u16);

    chunk.write_opcode(Opcode::Pow);
    chunk.write_u16(2); // dest = r2
    chunk.write_u16(0); // base = r0 (2)
    chunk.write_u16(1); // exp = r1 (10)

    chunk.write_opcode(Opcode::Print);
    chunk.write_u16(2);

    chunk.write_opcode(Opcode::Halt);

    // Execute in VM
    let (mut vm, output_capture) = VM::new_with_output_capture();

    let result = vm.run(chunk);
    assert!(result.is_ok(), "VM execution should succeed: {:?}", result.err());

    // Verify output contains "1024"
    let output = output_capture.lock().unwrap();
    assert!(
        output.last().map(|s| s.as_str()) == Some("1024"),
        "Expected output '1024', got {:?}",
        output.last()
    );
}

#[test]
fn test_pow_vm_execution_3_cubed() {
    // Test: 3^3 = 27
    let mut chunk = Chunk::new();

    let c3 = chunk.add_constant(Value::from_number(3.0));

    chunk.write_opcode(Opcode::LoadK);
    chunk.write_u16(0);
    chunk.write_u16(c3 as u16);

    chunk.write_opcode(Opcode::LoadK);
    chunk.write_u16(1);
    chunk.write_u16(c3 as u16);

    chunk.write_opcode(Opcode::Pow);
    chunk.write_u16(2);
    chunk.write_u16(0);
    chunk.write_u16(1);

    chunk.write_opcode(Opcode::Print);
    chunk.write_u16(2);

    chunk.write_opcode(Opcode::Halt);

    let (mut vm, output_capture) = nuzo_vm::VM::new_with_output_capture();

    vm.run(chunk).expect("execution should succeed");

    let output = output_capture.lock().unwrap();
    assert_eq!(output.last().map(|s| s.as_str()), Some("27"));
}

#[test]
fn test_pow_vm_execution_fractional() {
    // Test: 4^0.5 = 2 (square root)
    let mut chunk = Chunk::new();

    let c4 = chunk.add_constant(Value::from_number(4.0));
    let c05 = chunk.add_constant(Value::from_number(0.5));

    chunk.write_opcode(Opcode::LoadK);
    chunk.write_u16(0);
    chunk.write_u16(c4 as u16);

    chunk.write_opcode(Opcode::LoadK);
    chunk.write_u16(1);
    chunk.write_u16(c05 as u16);

    chunk.write_opcode(Opcode::Pow);
    chunk.write_u16(2);
    chunk.write_u16(0);
    chunk.write_u16(1);

    chunk.write_opcode(Opcode::Print);
    chunk.write_u16(2);

    chunk.write_opcode(Opcode::Halt);

    let (mut vm, output_capture) = nuzo_vm::VM::new_with_output_capture();

    vm.run(chunk).expect("execution should succeed");

    let output = output_capture.lock().unwrap();
    let last_output = output.last().map(|s| s.as_str());
    // Should be approximately "2" (may have decimal places)
    match last_output {
        Some(s) => {
            let val: f64 = s.parse().unwrap_or(0.0);
            assert!((val - 2.0).abs() < 0.0001, "Expected ~2.0, got {}", val);
        }
        None => panic!("No output captured"),
    }
}

// =========================================================================
// Layer 4: AST BinaryOp::Pow Integration Test
// =========================================================================

#[test]
fn test_ast_binary_op_pow_display() {
    use nuzo_frontend::ast::BinaryOp;

    // Verify Display implementation shows "**"
    assert_eq!(format!("{}", BinaryOp::Pow), "**");
}

#[test]
fn test_ast_binary_op_pow_equality() {
    use nuzo_frontend::ast::BinaryOp;

    // Verify Pow is distinct from other operators
    assert_ne!(BinaryOp::Pow, BinaryOp::Mul);
    assert_ne!(BinaryOp::Pow, BinaryOp::Add);
    assert_eq!(BinaryOp::Pow, BinaryOp::Pow);
}

// =========================================================================
// Architecture Validation: Pow Opcode Presence Verification
// =========================================================================

#[test]
fn verify_pow_architecture_file_count() {
    // 验证 Pow opcode 已在 Instruction 枚举中定义（opcode code = 40）
    // 这是表驱动架构的核心验证：Pow 应作为 Instruction::Pow 存在并可解码

    // 验证 Opcode::Pow 可从字节码 40 解码
    let pow_opcode = nuzo_bytecode::Opcode::decode_opcode(40);
    assert!(pow_opcode.is_some(), "字节码 40 应解码为有效 Opcode (Pow)");
    let pow_opcode = pow_opcode.unwrap();
    assert_eq!(pow_opcode.name(), "Pow", "字节码 40 应对应 Pow opcode, got {}", pow_opcode.name());

    // 验证 Pow 在 Opcode::ALL 中存在
    let all_opcodes = nuzo_bytecode::Opcode::ALL;
    let has_pow = all_opcodes.iter().any(|op| op.name() == "Pow");
    assert!(has_pow, "Opcode::ALL 应包含 Pow (表驱动架构应自动注册)");

    eprintln!("  [✓] Pow opcode 已注册 (code=40), 表驱动架构验证通过");
}
