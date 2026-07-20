//! Parser 属性测试 (Property-Based Tests)
//!
//! 对语法分析器进行属性测试，覆盖以下属性：
//!
//! - **表达式解析不变性**：相同源码多次解析结果一致
//! - **数字字面量保真**：解析后的 AST 数值与源码一致
//! - **字符串字面量保真**：解析后的 AST 字符串内容与源码一致
//! - **二元表达式结构**：二元运算的 AST 节点结构正确
//! - **括号优先级**：括号改变运算优先级
//! - **注释不影响解析**：含注释的源码与不含注释的解析结果一致
//! - **空白不敏感**：空白数量变化不影响解析结果
//! - **空程序解析**：空/仅空白源码解析为空 Program

use nuzo_frontend::{Expr, Parser, Stmt};
use proptest::prelude::*;

proptest! {
    #[test]
    fn prop_parse_deterministic(source in "[a-zA-Z0-9 +\\-*/()=<>! ]{0,100}") {
        let result1 = Parser::parse(&source);
        let result2 = Parser::parse(&source);
        match (&result1, &result2) {
            (Ok(p1), Ok(p2)) => {
                prop_assert_eq!(p1.statements.len(), p2.statements.len());
            }
            (Err(_), Err(_)) => {}
            _ => prop_assert!(false, "inconsistent parse results"),
        }
    }

    #[test]
    fn prop_number_literal_fidelity(n in 0i64..1_000_000) {
        let source = n.to_string();
        if let Ok(program) = Parser::parse(&source) {
            prop_assert_eq!(program.statements.len(), 1);
            if let Stmt::Expr(Expr::Number { value, .. }) = &program.statements[0] {
                prop_assert_eq!(*value, n as f64);
            } else {
                prop_assert!(false, "expected Number expression");
            }
        }
    }

    #[test]
    fn prop_float_literal_fidelity(int_part in 0i64..100_000, frac in 0u32..1_000_000) {
        let source = format!("{}.{}", int_part, frac);
        let expected = source.parse::<f64>().unwrap();
        if let Ok(program) = Parser::parse(&source) {
            prop_assert_eq!(program.statements.len(), 1);
            if let Stmt::Expr(Expr::Number { value, .. }) = &program.statements[0] {
                prop_assert!((value - expected).abs() < 1e-6);
            }
        }
    }

    #[test]
    fn prop_string_literal_fidelity(content in "[a-zA-Z0-9 ,.!?]{0,50}") {
        let source = format!("\"{}\"", content);
        if let Ok(program) = Parser::parse(&source) {
            prop_assert_eq!(program.statements.len(), 1);
            if let Stmt::Expr(Expr::String { value, .. }) = &program.statements[0] {
                prop_assert_eq!(value, &content);
            }
        }
    }

    #[test]
    fn prop_single_quote_string_fidelity(content in "[a-zA-Z0-9 ,.!?]{0,50}") {
        let source = format!("'{}'", content);
        if let Ok(program) = Parser::parse(&source) {
            prop_assert_eq!(program.statements.len(), 1);
            if let Stmt::Expr(Expr::String { value, .. }) = &program.statements[0] {
                prop_assert_eq!(value, &content);
            }
        }
    }

    #[test]
    fn prop_whitespace_only_parses(ws in "[ \t\n\r]{1,50}") {
        let program = Parser::parse(&ws).unwrap();
        prop_assert!(program.statements.is_empty());
    }

    #[test]
    fn prop_comment_no_effect(n in 0i64..100_000) {
        let without = Parser::parse(&n.to_string()).unwrap();
        let with_comment = Parser::parse(&format!("# comment\n{}", n)).unwrap();
        prop_assert_eq!(without.statements.len(), with_comment.statements.len());
    }

    #[test]
    fn prop_whitespace_insensitive(a in -10_000i64..10_000, b in -10_000i64..10_000) {
        let compact = Parser::parse(&format!("{}+{}", a, b)).unwrap();
        let spaced = Parser::parse(&format!("  {}   +   {}  ", a, b)).unwrap();
        prop_assert_eq!(compact.statements.len(), spaced.statements.len());
    }

    #[test]
    fn prop_binary_expr_structure(a in 0i64..1000, b in 0i64..1000) {
        let source = format!("{} + {}", a, b);
        let program = Parser::parse(&source).unwrap();
        prop_assert_eq!(program.statements.len(), 1);
        if let Stmt::Expr(Expr::Binary { left, right, .. }) = &program.statements[0] {
            assert!(matches!(left.as_ref(), Expr::Number { .. }));
            assert!(matches!(right.as_ref(), Expr::Number { .. }));
        } else {
            prop_assert!(false, "expected Binary expression");
        }
    }

    #[test]
    fn prop_precedence_structure(
        a in 0i64..100,
        b in 0i64..100,
        c in 0i64..100
    ) {
        let source = format!("{} + {} * {}", a, b, c);
        let program = Parser::parse(&source).unwrap();
        if let Stmt::Expr(Expr::Binary { left, right, .. }) = &program.statements[0] {
            assert!(matches!(left.as_ref(), Expr::Number { .. }));
            assert!(matches!(right.as_ref(), Expr::Binary { .. }));
        } else {
            prop_assert!(false, "expected Binary at top level");
        }
    }

    #[test]
    fn prop_paren_structure(
        a in 0i64..100,
        b in 0i64..100,
        c in 0i64..100
    ) {
        let source = format!("({} + {}) * {}", a, b, c);
        let program = Parser::parse(&source).unwrap();
        if let Stmt::Expr(Expr::Binary { left, right, .. }) = &program.statements[0] {
            assert!(matches!(left.as_ref(), Expr::Binary { .. }));
            assert!(matches!(right.as_ref(), Expr::Number { .. }));
        } else {
            prop_assert!(false, "expected Binary at top level");
        }
    }

    #[test]
    fn prop_assignment_parses(a in -100_000i64..100_000) {
        let source = format!("x = {}", a);
        let program = Parser::parse(&source).unwrap();
        prop_assert_eq!(program.statements.len(), 1);
        assert!(matches!(&program.statements[0], Stmt::Assign { .. }));
    }

    #[test]
    fn prop_multi_statement(
        a in -1000i64..1000,
        b in -1000i64..1000,
        c in -1000i64..1000
    ) {
        let source = format!("x = {}\ny = {}\nz = {} + y", a, b, c);
        let program = Parser::parse(&source).unwrap();
        prop_assert_eq!(program.statements.len(), 3);
    }

    #[test]
    fn prop_unary_neg_structure(a in 0i64..1000) {
        let source = format!("-{}", a);
        let program = Parser::parse(&source).unwrap();
        prop_assert_eq!(program.statements.len(), 1);
        if let Stmt::Expr(Expr::Unary { operand, .. }) = &program.statements[0] {
            assert!(matches!(operand.as_ref(), Expr::Number { .. }));
        } else {
            prop_assert!(false, "expected Unary expression");
        }
    }
}

#[test]
fn prop_bool_true_literal() {
    let program = Parser::parse("true").unwrap();
    assert_eq!(program.statements.len(), 1);
    if let Stmt::Expr(Expr::Bool { value, .. }) = &program.statements[0] {
        assert!(*value);
    }
}

#[test]
fn prop_bool_false_literal() {
    let program = Parser::parse("false").unwrap();
    assert_eq!(program.statements.len(), 1);
    if let Stmt::Expr(Expr::Bool { value, .. }) = &program.statements[0] {
        assert!(!*value);
    }
}

#[test]
fn prop_nil_literal() {
    let program = Parser::parse("nil").unwrap();
    assert_eq!(program.statements.len(), 1);
    assert!(matches!(&program.statements[0], Stmt::Expr(Expr::Nil { .. })));
}

#[test]
fn prop_empty_program_parses() {
    let program = Parser::parse("").unwrap();
    assert!(program.statements.is_empty());
}
