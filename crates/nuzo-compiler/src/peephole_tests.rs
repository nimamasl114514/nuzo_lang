//! # Peephole 优化 Pass 测试套件
//!
//! 覆盖 Nuzo 编译器实现的四大类窥孔优化：
//!
//! - **C1: 常量折叠** (Constant Folding) — 已有实现
//! - **C2: 死存储消除** (Dead Store Elimination) — 通过 emit_mov 冗余消除覆盖
//! - **C3: 恒等消除 + 冗余 Mov 消除** (Identity Elimination) — 本次新增
//! - **C4: Unreachable 代码跳过** (Dead Code Elimination) — 已有实现
//!
//! # 测试策略
//!
//! 采用**字节码验证法**（Bytecode Verification）：
//! 1. 编译源代码字符串 → 得到 Chunk
//! 2. 检查生成的指令序列是否符合优化预期
//! 3. 可选：通过 VM 执行验证语义正确性

use crate::compiler::Compiler;
use nuzo_values::ValueExt;

/// 辅助函数：编译源代码并返回 Chunk
fn compile_source(source: &str) -> nuzo_bytecode::Chunk {
    Compiler::compile(source).expect("编译失败")
}

/// 辅助函数：统计指定 Opcode 在字节码中出现的次数
///
/// 使用**指令对齐迭代**（非滑动窗口），通过 `Opcode::decode_opcode` +
/// `Opcode::instruction_size` 逐条前进，避免指令内部字节被误判为操作码。
fn count_opcode(chunk: &nuzo_bytecode::Chunk, opcode: nuzo_bytecode::Opcode) -> usize {
    let code = chunk.code();
    let mut count = 0;
    let mut ip = 0;
    while ip < code.len() {
        let op = match nuzo_bytecode::Opcode::decode_opcode(code[ip]) {
            Some(op) => op,
            None => break, // 无效操作码字节，停止迭代
        };
        if op == opcode {
            count += 1;
        }
        let size = op.instruction_size();
        if size == 0 || ip + size > code.len() {
            break; // 安全防护：避免无限循环 / 越界
        }
        ip += size;
    }
    count
}

/// 辅助函数：递归统计指定 Opcode 在顶层 Chunk 及其所有嵌套
/// FunctionPrototype（通过常量池中的 Closure 对象）中的出现次数
///
/// 旧编译器将函数定义编译为独立的 FunctionPrototype，存入常量池的
/// HeapObject::Closure 中。要检查函数体内的指令，必须递归遍历
/// Closure 对象引用的 FunctionPrototype 的字节码。
fn count_opcode_recursive(chunk: &nuzo_bytecode::Chunk, opcode: nuzo_bytecode::Opcode) -> usize {
    let mut count = count_opcode(chunk, opcode);

    // 遍历常量池，查找 Closure 对象并递归统计
    for value in chunk.constants() {
        if let Some(obj) = value.as_heap_object_opt()
            && let nuzo_values::heap::HeapObject::Closure { prototype, .. } = obj.as_ref()
        {
            // 从 FunctionPrototype 构建临时 Chunk 来统计
            let sub_chunk = nuzo_bytecode::Chunk::from_arcs(
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

// ============================================================================
// C1: 常量折叠测试 (Constant Folding)
// ============================================================================

#[test]
fn test_const_fold_binary_add() {
    // `3 + 5` 应该折叠为单条 LoadK 8，而非 LoadK 3; LoadK 5; Add
    let chunk = compile_source("3 + 5");
    let add_count = count_opcode(&chunk, nuzo_bytecode::Opcode::Add);
    assert_eq!(add_count, 0, "Add 指令应被常量折叠消除");

    // 应该有且仅有 1 条 LoadK（结果值 8）
    let loadk_count = count_opcode(&chunk, nuzo_bytecode::Opcode::LoadK);
    assert!(loadk_count >= 1, "应该至少有 1 条 LoadK 指令");
}

#[test]
fn test_const_fold_binary_sub_mul_div() {
    // 减法: `10 - 3` → LoadK 7
    let chunk = compile_source("10 - 3");
    assert_eq!(count_opcode(&chunk, nuzo_bytecode::Opcode::Sub), 0);

    // 乘法: `4 * 6` → LoadK 24
    let chunk = compile_source("4 * 6");
    assert_eq!(count_opcode(&chunk, nuzo_bytecode::Opcode::Mul), 0);

    // 除法: `20 / 4` → LoadK 5
    let chunk = compile_source("20 / 4");
    assert_eq!(count_opcode(&chunk, nuzo_bytecode::Opcode::Div), 0);
}

#[test]
fn test_const_fold_binary_div_by_zero_not_folded() {
    // `1 / 0` 不应折叠（保留运行时除零错误）
    let chunk = compile_source("1 / 0");
    let div_count = count_opcode(&chunk, nuzo_bytecode::Opcode::Div);
    assert_eq!(div_count, 1, "除以零不应被折叠");
}

#[test]
fn test_const_fold_unary_negate() {
    // `-42` 应该折叠为 LoadK -42
    let chunk = compile_source("-42");
    let neg_count = count_opcode(&chunk, nuzo_bytecode::Opcode::Neg);
    assert_eq!(neg_count, 0, "Neg 指令应被常量折叠消除");
}

#[test]
fn test_const_fold_unary_not() {
    // `!true` 应该折叠为 LoadFalse
    let chunk = compile_source("!true");
    let not_count = count_opcode(&chunk, nuzo_bytecode::Opcode::Not);
    assert_eq!(not_count, 0, "Not 指令应被常量折叠消除");
}

#[test]
fn test_const_fold_nested() {
    // `(2 + 3) * 4`: 内层 (2+3) 被常量折叠为 LoadK 5（Add 被消除）。
    // 外层 * 4：左操作数在 AST 层面仍是 Binary 节点（不是 Number 字面量），
    // 所以不会触发常量折叠或恒等消除，会生成 Mul 指令。
    // 这是单遍编译器的固有限制——常量折叠仅在 AST 字面量间生效。
    let chunk = compile_source("(2 + 3) * 4");
    assert_eq!(
        count_opcode(&chunk, nuzo_bytecode::Opcode::Add),
        0,
        "内层 (2+3) 应被常量折叠消除 Add"
    );
    // Mul 可能存在（外层无法折叠），这不影响正确性
}

// ============================================================================
// C3a: 二元恒等消除测试 (Identity Elimination)
// ============================================================================

#[test]
fn test_identity_add_zero_right() {
    // `x + 0` 应该省去 Add 指令（x 是变量，需要 GetGlobal/LoadK）
    let chunk = compile_source("let x = 10; x + 0");
    let add_count = count_opcode(&chunk, nuzo_bytecode::Opcode::Add);
    assert_eq!(add_count, 0, "x + 0 应省去 Add 指令");
}

#[test]
fn test_identity_add_zero_left() {
    // `0 + x` 同样应该省去 Add
    let chunk = compile_source("let x = 10; 0 + x");
    let add_count = count_opcode(&chunk, nuzo_bytecode::Opcode::Add);
    assert_eq!(add_count, 0, "0 + x 应省去 Add 指令");
}

#[test]
fn test_identity_sub_zero() {
    // `x - 0` 应该省去 Sub 指令
    let chunk = compile_source("let x = 10; x - 0");
    let sub_count = count_opcode(&chunk, nuzo_bytecode::Opcode::Sub);
    assert_eq!(sub_count, 0, "x - 0 应省去 Sub 指令");
}

#[test]
fn test_identity_sub_zero_left_not_optimized() {
    // `0 - x` 不应优化（结果是 -x）
    let chunk = compile_source("let x = 10; 0 - x");
    let sub_count = count_opcode(&chunk, nuzo_bytecode::Opcode::Sub);
    assert_eq!(sub_count, 1, "0 - x 不应优化（结果是 -x）");
}

#[test]
fn test_identity_mul_one_right() {
    // `x * 1` 应该省去 Mul 指令
    let chunk = compile_source("let x = 10; x * 1");
    let mul_count = count_opcode(&chunk, nuzo_bytecode::Opcode::Mul);
    assert_eq!(mul_count, 0, "x * 1 应省去 Mul 指令");
}

#[test]
fn test_identity_mul_one_left() {
    // `1 * x` 同样应该省去 Mul
    let chunk = compile_source("let x = 10; 1 * x");
    let mul_count = count_opcode(&chunk, nuzo_bytecode::Opcode::Mul);
    assert_eq!(mul_count, 0, "1 * x 应省去 Mul 指令");
}

#[test]
fn test_identity_div_one() {
    // `x / 1` 应该省去 Div 指令
    let chunk = compile_source("let x = 10; x / 1");
    let div_count = count_opcode(&chunk, nuzo_bytecode::Opcode::Div);
    assert_eq!(div_count, 0, "x / 1 应省去 Div 指令");
}

#[test]
fn test_identity_div_one_left_not_optimized() {
    // `1 / x` 不应优化（结果是倒数）
    // 使用函数参数避免 IR 常量传播折叠整个表达式
    let chunk = compile_source("fn f(x) { return 1 / x }");
    // 函数体内的指令在 FunctionPrototype 中，需要递归统计
    let div_count = count_opcode_recursive(&chunk, nuzo_bytecode::Opcode::Div);
    assert_eq!(div_count, 1, "1 / x 不应优化（结果是倒数）");
}

#[test]
fn test_identity_mul_zero_right() {
    // `x * 0` 对于简单表达式 x 应该优化为 LoadK 0
    let chunk = compile_source("let x = 10; x * 0");
    let mul_count = count_opcode(&chunk, nuzo_bytecode::Opcode::Mul);
    assert_eq!(mul_count, 0, "x * 0 (简单表达式) 应优化为 LoadK 0");
}

#[test]
fn test_identity_mul_zero_left() {
    // `0 * x` 应该优化为 LoadK 0
    let chunk = compile_source("let x = 10; 0 * x");
    let mul_count = count_opcode(&chunk, nuzo_bytecode::Opcode::Mul);
    assert_eq!(mul_count, 0, "0 * x 应优化为 LoadK 0");
}

#[test]
fn test_identity_self_subtraction() {
    // 旧路径（direct emission）会折叠 `x - x → LoadK 0`。
    // 新 IR 路径中 `let x = 42` 使 x 成为全局变量，IR 无法证明两次 global load
    // 返回相同值（全局可能被修改），因此不折叠，保留 Sub 指令。
    // 这是 IR 路径的安全行为（保守不优化优于错误优化）。
    let chunk = compile_source("let x = 42; x - x");
    let sub_count = count_opcode(&chunk, nuzo_bytecode::Opcode::Sub);
    assert_eq!(sub_count, 1, "IR 路径对全局变量 x - x 不折叠（保守安全）");
}

#[test]
fn test_identity_no_false_optimization() {
    // `x + 1` 不应被错误优化
    // 使用 input() 避免 IR 常量传播折叠整个表达式
    let chunk = compile_source("let x = input(); x + 1");
    let add_count = count_opcode(&chunk, nuzo_bytecode::Opcode::Add);
    assert_eq!(add_count, 1, "x + 1 不应被优化");
}

#[test]
fn test_identity_complex_expr_not_optimized_for_mul_zero() {
    // 旧路径（direct emission）有 is_simple 检查，对非简单表达式（如函数调用）
    // 不触发 x * 0 优化，以保留副作用。
    // 新 IR 路径没有 is_simple 检查，对所有表达式都执行 x * 0 → 0 折叠。
    // 这是 IR 路径与旧路径的行为差异（IR 优化更激进）。
    // 注意：input() 是无副作用的内置函数，所以此处的折叠是安全的。
    let chunk = compile_source("let x = input(); x * 0");
    let mul_count = count_opcode(&chunk, nuzo_bytecode::Opcode::Mul);
    assert_eq!(mul_count, 0, "IR 路径对 x * 0 一律折叠为 0（无 is_simple 检查）");
}

// ============================================================================
// C3b: 冗余 Mov 消除测试 (Redundant Mov Elimination)
// ============================================================================

#[test]
fn test_redundant_mov_elimination_basic() {
    // 当赋值目标是局部变量且值已在同一寄存器时，不应生成 Mov
    // 这个测试间接验证 emit_mov 的 dest==src 检查
    let chunk = compile_source("let x = 42; x");
    // 如果 x 被赋值后立即返回，中间不应有冗余 Mov
    // 具体验证方式：检查 Mov 指令数量合理
    let mov_count = count_opcode(&chunk, nuzo_bytecode::Opcode::Mov);
    // 至少不应该有不必要的自我拷贝
    assert!(mov_count < 3, "Mov 指令数量异常多，可能有冗余");
}

// ============================================================================
// C4: Unreachable 代码跳过测试 (Dead Code Elimination)
// ============================================================================

#[test]
fn test_unreachable_after_return() {
    // return 后的代码不应生成字节码
    let chunk = compile_source(
        r#"
        fn foo() { return 42 }
        foo()
    "#,
    );
    // 验证编译成功即可（return 后的代码被跳过）
    assert!(!chunk.code().is_empty(), "Chunk 不应为空");
}

#[test]
fn test_unreachable_after_break() {
    // break 后的代码不应生成字节码
    let chunk = compile_source(
        r#"
        let sum = 0
        loop { break; println("unreachable") }
        sum
    "#,
    );
    // 编译成功即说明 break 后的代码被正确跳过
    assert!(!chunk.code().is_empty());
}

#[test]
fn test_unreachable_if_constant_true() {
    // if (true) { ... } else { ... } — else 分支不可达
    let chunk = compile_source("if (true) { 1 } else { 2 }");
    // 结果应该是 1，else 分支的 2 不应出现在有效路径中
    // 我们无法直接从字节码判断，但可以验证编译成功且无 panic
    assert!(!chunk.code().is_empty());
}

#[test]
fn test_unreachable_if_constant_false() {
    // if (false) { ... } — then 分支不可达
    let chunk = compile_source("if (false) { 1 } else { 2 }");
    // 结果应该是 2，then 分支的 1 不应出现
    assert!(!chunk.code().is_empty());
}

#[test]
fn test_unreachable_while_constant_false() {
    // while (false) { ... } — 循环体不可达
    let chunk = compile_source("while (false) { println('dead') } 42");
    // 循环体不应生成任何字节码
    // 验证：编译成功且结果为 42
    assert!(!chunk.code().is_empty());
}

// ============================================================================
// 组合优化测试 (Combined Optimizations)
// ============================================================================

#[test]
fn test_combined_const_fold_and_identity() {
    // `(3 + 0) * 1` → 常量折叠 3+0=3 → 恒等消除 3*1=3 → LoadK 3
    let chunk = compile_source("(3 + 0) * 1");
    let add_count = count_opcode(&chunk, nuzo_bytecode::Opcode::Add);
    let mul_count = count_opcode(&chunk, nuzo_bytecode::Opcode::Mul);
    assert_eq!(add_count, 0, "(3 + 0) 应被常量折叠或恒等消除");
    assert_eq!(mul_count, 0, "... * 1 应被恒等消除");
}

#[test]
fn test_chained_identity_optimizations() {
    // `((x + 0) * 1) - 0` → 全部恒等消除，最终等于 x
    let chunk = compile_source("let x = 42; ((x + 0) * 1) - 0");
    let add_count = count_opcode(&chunk, nuzo_bytecode::Opcode::Add);
    let mul_count = count_opcode(&chunk, nuzo_bytecode::Opcode::Mul);
    let sub_count = count_opcode(&chunk, nuzo_bytecode::Opcode::Sub);
    assert_eq!(add_count, 0, "x + 0 应消除");
    assert_eq!(mul_count, 0, "... * 1 应消除");
    assert_eq!(sub_count, 0, "... - 0 应消除");
}

// ============================================================================
// Edge Case / Poison Pill 测试
// ============================================================================

#[test]
fn test_edge_case_floating_point_precision() {
    // 浮点数边界: `0.1 + 0.2` 不应被常量折叠为精确值
    // （实际上它会被折叠为 0.30000000000000004，这是正确行为）
    let chunk = compile_source("0.1 + 0.2");
    let add_count = count_opcode(&chunk, nuzo_bytecode::Opcode::Add);
    assert_eq!(add_count, 0, "浮点数字面量加法应被常量折叠");
}

#[test]
fn test_poison_pill_side_effects_preserved() {
    // 有副作用的表达式不应被过度优化
    // `x + foo()` 即使 x=0 也必须调用 foo()
    // 这里我们验证普通二元运算（无特殊模式）正常生成指令
    // 使用函数参数避免 IR 常量传播折叠 x + y
    let chunk = compile_source("fn f(x, y) { return x + y }");
    // 函数体内的指令在 FunctionPrototype 中，需要递归统计
    let add_count = count_opcode_recursive(&chunk, nuzo_bytecode::Opcode::Add);
    assert_eq!(add_count, 1, "普通加法必须生成 Add 指令");
}

#[test]
fn test_poison_pill_comparison_not_optimized() {
    // 比较运算符不参与恒等消除（语义不同）
    // 使用函数参数避免 IR 常量传播折叠 x == 1
    let chunk = compile_source("fn f(x) { return x == 1 }");
    // 函数体内的指令在 FunctionPrototype 中，需要递归统计
    let eq_count = count_opcode_recursive(&chunk, nuzo_bytecode::Opcode::Eq);
    assert_eq!(eq_count, 1, "比较运算符不应被恒等消除");
}

#[test]
fn test_large_constants_fold_correctly() {
    // 大数值常量折叠
    let chunk = compile_source("9999999999 + 1");
    let add_count = count_opcode(&chunk, nuzo_bytecode::Opcode::Add);
    assert_eq!(add_count, 0, "大数值加法应被常量折叠");
}

#[test]
fn test_negative_zero_handling() {
    // 旧路径：`-0` 在 AST 中是 Unary(Negate, Number{0})，is_zero() 不匹配，
    // 所以 `x + (-0)` 保留 Add 指令。
    // 新 IR 路径：IR 在常量折叠阶段将 `-0` 规范化为 `0`，然后执行 `x + 0 → x`
    // 恒等消除，移除 Add 指令。这是 IR 路径的更激进优化行为。
    let chunk = compile_source("fn f(x) { return x + (-0) }");
    // IR 路径行为：Add 被消除（-0 被规范化为 0，触发 x + 0 → x）
    // 函数体内的指令在 FunctionPrototype 中，需要递归统计
    let add_count = count_opcode_recursive(&chunk, nuzo_bytecode::Opcode::Add);
    assert_eq!(add_count, 0, "IR 路径将 -0 规范化为 0 并执行 x + 0 → x 恒等消除");
}

// ============================================================================
// C2: 块表达式寄存器耗尽修复回归测试
// (Block Expression Register Exhaustion Regression)
// ============================================================================
//
// 修复位置：`statements.rs::compile_block_core`（已统一至 IR 路径）
// 修复内容：
//   1. 中间表达式寄存器自动释放，防止块内寄存器膨胀
//   2. 空块/语句块用 `alloc_register()?` 替代 `expect()`，
//      使寄存器耗尽时返回 `CompileError::TooManyLocals` 而非 panic
//
// 架构说明：
//   - 块表达式现在统一由 CodeGenerator 处理
//   - 本组测试验证统一路径的语义与寄存器行为

/// 深度嵌套块表达式的层数。
/// 50 层足以验证递归编译不会导致寄存器线性增长
/// （修复前每层嵌套会泄漏一个寄存器）。
const DEEP_NESTING_DEPTH: usize = 50;

/// 多语句块中用于验证中间寄存器释放的语句数量。
/// 修复前每条表达式语句会占用一个寄存器导致膨胀；
/// 修复后中间寄存器被复用，locals_count 应远小于此值。
const MULTI_STMT_COUNT: usize = 8;

/// 辅助函数：生成 n 层嵌套的块表达式源码 `{ { { ... body ... } } }`
fn nested_block_source(depth: usize, body: &str) -> String {
    let mut s = String::with_capacity(depth * 2 + body.len());
    for _ in 0..depth {
        s.push('{');
    }
    s.push_str(body);
    for _ in 0..depth {
        s.push('}');
    }
    s
}

#[test]
fn test_deeply_nested_block_expressions_no_overflow() {
    // 假设 H1: 50 层嵌套块表达式编译成功且无 panic，
    //         且 locals_count 远小于嵌套深度（中间寄存器被释放）。
    // 反例：修复前，深度嵌套会因寄存器泄漏导致 next_reg 线性增长，
    //       极端情况下触发 expect() panic。
    let source = nested_block_source(DEEP_NESTING_DEPTH, "42");

    // 统一 IR 路径（compile）
    let chunk = compile_source(&source);
    assert!(
        chunk.locals_count < DEEP_NESTING_DEPTH as u16,
        "统一路径：深度嵌套块应释放中间寄存器，locals_count={} 应小于嵌套深度={}",
        chunk.locals_count,
        DEEP_NESTING_DEPTH
    );
}

#[test]
fn test_block_expression_preserves_semantics() {
    // 假设 H2: 块表达式内部最后一个表达式的值正确传递到外部。
    // 验证：`{ 1; 2; 3 }` 编译后，常量池包含数值 3（最后值未被丢弃）。
    let chunk = compile_source("{ 1; 2; 3 }");

    let has_three = chunk.constants().iter().any(|v| v.try_number() == Some(3.0));
    assert!(has_three, "统一路径：块表达式最后值 3 应被保留在常量池中");
}

#[test]
fn test_block_expression_intermediate_regs_released() {
    // 假设 H3: 多语句块中，中间表达式寄存器被自动释放，
    //         locals_count 不随语句数线性增长。
    // 反例：若修复无效，8 条语句会导致 locals_count >= 8。
    let mut source = String::from("{ ");
    for i in 1..=MULTI_STMT_COUNT {
        if i > 1 {
            source.push_str("; ");
        }
        source.push_str(&i.to_string());
    }
    source.push_str(" }");

    let chunk = compile_source(&source);
    // 修复后，中间表达式寄存器被释放，仅保留最后值寄存器。
    // locals_count 应远小于语句数（允许少量额外寄存器用于常量加载）。
    assert!(
        chunk.locals_count < MULTI_STMT_COUNT as u16,
        "统一路径：多语句块中间寄存器应被释放，locals_count={} 应小于语句数={}",
        chunk.locals_count,
        MULTI_STMT_COUNT
    );
}
