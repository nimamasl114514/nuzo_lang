//! 编译器 + VM 端到端属性测试 (Property-Based Tests)
//!
//! 验证源码 → 编译 → 执行的完整流水线正确性，覆盖以下属性：
//!
//! - **整数加法正确性**：Nuzo 程序 `a + b` 的结果与 Rust 整数加法一致
//! - **整数乘法正确性**：Nuzo 程序 `a * b` 的结果与 Rust 整数乘法一致
//! - **整数减法正确性**：Nuzo 程序 `a - b` 的结果与 Rust 整数减法一致
//! - **常量折叠正确性**：编译期常量表达式与运行时求值一致
//! - **变量赋值回读**：赋值后变量值与赋值一致
//! - **链式加法正确性**：多操作数加法链结果正确
//! - **比较运算正确性**：比较结果与 Rust 比较一致
//! - **幂运算正确性**：小指数幂运算结果正确
//! - **嵌套表达式正确性**：带括号的嵌套表达式优先级正确
//! - **字符串拼接正确性**：字符串拼接结果与预期一致
//! - **一元负号正确性**：`-a` 与 `0 - a` 结果一致
//! - **空程序安全性**：空程序或仅空白程序编译运行不崩溃

use nuzo_compiler::Compiler;
use nuzo_values::ValueExt;
use nuzo_vm::VM;
use proptest::prelude::*;

fn run(source: &str) -> String {
    let chunk = Compiler::compile(source).expect("编译失败");
    let mut vm = VM::new();
    let result = vm.run(chunk).expect("VM 执行失败");
    result.concat_repr()
}

fn run_as_number(source: &str) -> f64 {
    let chunk = Compiler::compile(source).expect("编译失败");
    let mut vm = VM::new();
    let result = vm.run(chunk).expect("VM 执行失败");
    result.as_number()
}

proptest! {
    #[test]
    fn prop_int_add_correct(a in -100_000i64..100_000, b in -100_000i64..100_000) {
        let source = format!("{} + {}", a, b);
        let result = run_as_number(&source);
        prop_assert_eq!(result, (a + b) as f64);
    }

    #[test]
    fn prop_int_sub_correct(a in -100_000i64..100_000, b in -100_000i64..100_000) {
        let source = format!("{} - {}", a, b);
        let result = run_as_number(&source);
        prop_assert_eq!(result, (a - b) as f64);
    }

    #[test]
    fn prop_int_mul_correct(a in -10_000i64..10_000, b in -10_000i64..10_000) {
        let source = format!("{} * {}", a, b);
        let result = run_as_number(&source);
        prop_assert_eq!(result, (a * b) as f64);
    }

    #[test]
    fn prop_int_div_correct(a in -100_000i64..100_000, b in 1i64..100_000) {
        let source = format!("{} / {}", a, b);
        let result = run_as_number(&source);
        if a % b == 0 {
            prop_assert_eq!(result, (a / b) as f64);
        } else {
            prop_assert!((result - (a as f64 / b as f64)).abs() < 1e-6);
        }
    }

    #[test]
    fn prop_int_mod_correct(a in 0i64..100_000, b in 1i64..100_000) {
        let source = format!("{} % {}", a, b);
        let result = run_as_number(&source);
        prop_assert_eq!(result, (a % b) as f64);
    }

    #[test]
    fn prop_chain_add_correct(
        a in -10_000i64..10_000,
        b in -10_000i64..10_000,
        c in -10_000i64..10_000
    ) {
        let source = format!("{} + {} + {}", a, b, c);
        let result = run_as_number(&source);
        prop_assert_eq!(result, (a + b + c) as f64);
    }

    #[test]
    fn prop_chain_mul_correct(
        a in -100i64..100,
        b in -100i64..100,
        c in -100i64..100
    ) {
        let source = format!("{} * {} * {}", a, b, c);
        let result = run_as_number(&source);
        prop_assert_eq!(result, (a * b * c) as f64);
    }

    #[test]
    fn prop_precedence_mul_over_add(
        a in -1000i64..1000,
        b in -1000i64..1000,
        c in -1000i64..1000
    ) {
        let source = format!("{} + {} * {}", a, b, c);
        let result = run_as_number(&source);
        prop_assert_eq!(result, (a + b * c) as f64);
    }

    #[test]
    fn prop_paren_priority(
        a in -100i64..100,
        b in -100i64..100,
        c in -100i64..100
    ) {
        let source = format!("({} + {}) * {}", a, b, c);
        let result = run_as_number(&source);
        prop_assert_eq!(result, ((a + b) * c) as f64);
    }

    #[test]
    fn prop_eq_comparison(a in -10_000i64..10_000, b in -10_000i64..10_000) {
        let source = format!("{} == {}", a, b);
        let result = run(&source);
        let expected = if a == b { "true" } else { "false" };
        prop_assert_eq!(result, expected);
    }

    #[test]
    fn prop_neq_comparison(a in -10_000i64..10_000, b in -10_000i64..10_000) {
        let source = format!("{} != {}", a, b);
        let result = run(&source);
        let expected = if a != b { "true" } else { "false" };
        prop_assert_eq!(result, expected);
    }

    #[test]
    fn prop_lt_comparison(a in -10_000i64..10_000, b in -10_000i64..10_000) {
        let source = format!("{} < {}", a, b);
        let result = run(&source);
        let expected = if a < b { "true" } else { "false" };
        prop_assert_eq!(result, expected);
    }

    #[test]
    fn prop_gt_comparison(a in -10_000i64..10_000, b in -10_000i64..10_000) {
        let source = format!("{} > {}", a, b);
        let result = run(&source);
        let expected = if a > b { "true" } else { "false" };
        prop_assert_eq!(result, expected);
    }

    #[test]
    fn prop_lte_comparison(a in -10_000i64..10_000, b in -10_000i64..10_000) {
        let source = format!("{} <= {}", a, b);
        let result = run(&source);
        let expected = if a <= b { "true" } else { "false" };
        prop_assert_eq!(result, expected);
    }

    #[test]
    fn prop_gte_comparison(a in -10_000i64..10_000, b in -10_000i64..10_000) {
        let source = format!("{} >= {}", a, b);
        let result = run(&source);
        let expected = if a >= b { "true" } else { "false" };
        prop_assert_eq!(result, expected);
    }

    #[test]
    fn prop_unary_neg_correct(a in -100_000i64..100_000) {
        let source = format!("-{}", a);
        let result = run_as_number(&source);
        prop_assert_eq!(result, (-a) as f64);
    }

    #[test]
    fn prop_double_neg_correct(a in -100_000i64..100_000) {
        let source = format!("-(-{})", a);
        let result = run_as_number(&source);
        prop_assert_eq!(result, a as f64);
    }

    #[test]
    fn prop_variable_assign_readback(a in -100_000i64..100_000) {
        let source = format!("x = {}\nx", a);
        let result = run_as_number(&source);
        prop_assert_eq!(result, a as f64);
    }

    #[test]
    fn prop_variable_in_expression(
        a in -10_000i64..10_000,
        b in -10_000i64..10_000
    ) {
        let source = format!("x = {}\ny = {}\nx + y", a, b);
        let result = run_as_number(&source);
        prop_assert_eq!(result, (a + b) as f64);
    }

    #[test]
    fn prop_pow_small_int(base in -20i64..20, exp in 0u32..10) {
        let source = format!("{} ** {}", base, exp);
        let result = run_as_number(&source);
        let expected = (base as f64).powi(exp as i32);
        prop_assert!((result - expected).abs() < 1e-3);
    }

    #[test]
    fn prop_string_concat(a in "[a-zA-Z]{1,20}", b in "[a-zA-Z]{1,20}") {
        let source = format!("\"{}\" + \"{}\"", a, b);
        let result = run(&source);
        prop_assert_eq!(result, format!("{}{}", a, b));
    }

    #[test]
    fn prop_string_number_concat(s in "[a-zA-Z]{1,20}", n in 1i64..1000) {
        let source = format!("\"{}\" + {}", s, n);
        let result = run(&source);
        prop_assert_eq!(result, format!("{}{}", s, n));
    }

    #[test]
    fn prop_constant_folding_add(a in -10_000i64..10_000, b in -10_000i64..10_000) {
        let source = format!("{} + {}", a, b);
        let result = run_as_number(&source);
        prop_assert_eq!(result, (a + b) as f64);
    }

    #[test]
    fn prop_nested_parens(
        a in -100i64..100,
        b in -100i64..100,
        c in -100i64..100,
        d in -100i64..100
    ) {
        let source = format!("(({} + {}) * ({} - {}))", a, b, c, d);
        let result = run_as_number(&source);
        prop_assert_eq!(result, ((a + b) * (c - d)) as f64);
    }

    #[test]
    fn prop_empty_program_safe(ws in "[ \t\n\r]{0,50}") {
        let source = ws.to_string();
        let chunk = Compiler::compile(&source);
        prop_assert!(chunk.is_ok(), "empty/whitespace program should compile");
    }

    #[test]
    fn prop_single_number_literal(a in -1_000_000i64..1_000_000) {
        let source = a.to_string();
        let result = run_as_number(&source);
        prop_assert_eq!(result, a as f64);
    }

    #[test]
    fn prop_float_literal(int_part in 0i64..100_000, frac in 0u32..1_000_000) {
        let source = format!("{}.{}", int_part, frac);
        let result = run_as_number(&source);
        let expected = format!("{}.{}", int_part, frac).parse::<f64>().unwrap();
        prop_assert!((result - expected).abs() < 1e-6);
    }

    #[test]
    fn prop_mixed_add_mul(
        a in -100i64..100,
        b in -100i64..100,
        c in -100i64..100,
        d in -100i64..100
    ) {
        let source = format!("{} + {} * {} - {}", a, b, c, d);
        let result = run_as_number(&source);
        prop_assert_eq!(result, (a + b * c - d) as f64);
    }
}
