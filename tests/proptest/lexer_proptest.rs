//! Lexer 属性测试 (Property-Based Tests)
//!
//! 使用 proptest 对词法分析器进行随机化测试，覆盖以下属性：
//!
//! - **EOF 不变性**：任何输入扫描后最后一个 Token 始终为 Eof
//! - **零拷贝引用合法性**：所有 Token 文本切片是源码的子串
//! - **纯空白输入 → 仅 EOF**：仅含空白/注释的源码只产生 Eof
//! - **数字回送**：合法数字字面量扫描后文本与输入一致
//! - **标识符回送**：合法标识符扫描后文本与输入一致
//! - **字符串回送**：合法字符串字面量内容部分与输入一致
//! - **Token 计数确定性**：同一输入多次扫描结果完全一致
//! - **运算符覆盖**：随机组合运算符序列都能被正确扫描

use nuzo_frontend::{Lexer, TokenKind};
use proptest::prelude::*;

proptest! {
    #[test]
    fn prop_eof_always_last(input in ".{0,200}") {
        let tokens = Lexer::new(&input).scan_all();
        if let Ok(tokens) = tokens {
            prop_assert!(!tokens.is_empty());
            let last = tokens.last().unwrap();
            prop_assert_eq!(last.0.kind, TokenKind::Eof);
        }
    }

    #[test]
    fn prop_token_text_is_substring(input in ".{0,200}") {
        let tokens = Lexer::new(&input).scan_all();
        if let Ok(tokens) = tokens {
            for (_tok, text) in &tokens {
                if !text.is_empty() {
                    prop_assert!(
                        input.contains(*text),
                        "token text '{}' not found in source",
                        text
                    );
                }
            }
        }
    }

    #[test]
    fn prop_whitespace_only_yields_eof(
        ws in prop::collection::vec(
            proptest::sample::select(vec![" ", "\t", "\n", "\r", "  \n  "]),
            0..20
        )
    ) {
        let input: String = ws.concat();
        let tokens = Lexer::new(&input).scan_all().unwrap();
        prop_assert_eq!(tokens.len(), 1);
        prop_assert_eq!(tokens[0].0.kind, TokenKind::Eof);
    }

    #[test]
    fn prop_comments_skipped(comment in ".{0,100}") {
        let safe_comment: String = comment.chars().filter(|c| *c != '\n').collect();
        let source = format!("# {}\n42", safe_comment);
        let tokens = Lexer::new(&source).scan_all().unwrap();
        let non_eof: Vec<_> = tokens.iter().filter(|(t, _)| t.kind != TokenKind::Eof).collect();
        prop_assert_eq!(non_eof.len(), 1);
        prop_assert_eq!(non_eof[0].0.kind, TokenKind::Number);
        prop_assert_eq!(non_eof[0].1, "42");
    }

    #[test]
    fn prop_integer_roundtrip(n in 0i64..100_000_000) {
        let source = n.to_string();
        let tokens = Lexer::new(&source).scan_all().unwrap();
        let non_eof: Vec<_> = tokens.iter().filter(|(t, _)| t.kind != TokenKind::Eof).collect();
        prop_assert_eq!(non_eof.len(), 1);
        prop_assert_eq!(non_eof[0].0.kind, TokenKind::Number);
        prop_assert_eq!(non_eof[0].1, source.as_str());
    }

    #[test]
    fn prop_float_roundtrip(int_part in 0i64..100_000, frac_part in 0u32..1_000_000) {
        let source = format!("{}.{}", int_part, frac_part);
        let tokens = Lexer::new(&source).scan_all().unwrap();
        let non_eof: Vec<_> = tokens.iter().filter(|(t, _)| t.kind != TokenKind::Eof).collect();
        prop_assert_eq!(non_eof.len(), 1);
        prop_assert_eq!(non_eof[0].0.kind, TokenKind::Number);
        prop_assert_eq!(non_eof[0].1, source.as_str());
    }

    #[test]
    fn prop_identifier_roundtrip(
        first in "[a-zA-Z_]",
        rest in "[a-zA-Z0-9_]{0,30}"
    ) {
        let ident = format!("{}{}", first, rest);
        if nuzo_frontend::token::lookup_keyword(&ident).is_some() {
            return Ok(());
        }
        let tokens = Lexer::new(&ident).scan_all().unwrap();
        let non_eof: Vec<_> = tokens.iter().filter(|(t, _)| t.kind != TokenKind::Eof).collect();
        prop_assert_eq!(non_eof.len(), 1);
        prop_assert_eq!(non_eof[0].0.kind, TokenKind::Ident);
        prop_assert_eq!(non_eof[0].1, ident.as_str());
    }

    #[test]
    fn prop_string_double_quote_roundtrip(
        content in "[a-zA-Z0-9 ,.!?]{0,50}"
    ) {
        let source = format!("\"{}\"", content);
        let tokens = Lexer::new(&source).scan_all().unwrap();
        let non_eof: Vec<_> = tokens.iter().filter(|(t, _)| t.kind != TokenKind::Eof).collect();
        prop_assert_eq!(non_eof.len(), 1);
        prop_assert_eq!(non_eof[0].0.kind, TokenKind::String);
        prop_assert_eq!(non_eof[0].1, content.as_str());
    }

    #[test]
    fn prop_string_single_quote_roundtrip(
        content in "[a-zA-Z0-9 ,.!?]{0,50}"
    ) {
        let source = format!("'{}'", content);
        let tokens = Lexer::new(&source).scan_all().unwrap();
        let non_eof: Vec<_> = tokens.iter().filter(|(t, _)| t.kind != TokenKind::Eof).collect();
        prop_assert_eq!(non_eof.len(), 1);
        prop_assert_eq!(non_eof[0].0.kind, TokenKind::String);
        prop_assert_eq!(non_eof[0].1, content.as_str());
    }

    #[test]
    fn prop_scan_deterministic(input in "[a-zA-Z0-9 +\\-*/=<>!&|]{0,200}") {
        let tokens1 = Lexer::new(&input).scan_all();
        let tokens2 = Lexer::new(&input).scan_all();
        match (&tokens1, &tokens2) {
            (Ok(t1), Ok(t2)) => {
                prop_assert_eq!(t1.len(), t2.len());
                for ((a, ta), (b, tb)) in t1.iter().zip(t2.iter()) {
                    prop_assert_eq!(a.kind, b.kind);
                    prop_assert_eq!(ta, tb);
                }
            }
            (Err(_), Err(_)) => {}
            _ => prop_assert!(false, "inconsistent scan results for same input"),
        }
    }

    #[test]
    fn prop_single_operator(op in proptest::sample::select(vec![
        "+", "-", "*", "/", "%", "=", "==", "!=", "<", ">", "<=", ">=",
        "&&", "||", "=>", "+=", "-=", "*=", "/=", "??", "..", "..<", "|>",
    ])) {
        let tokens = Lexer::new(op).scan_all().unwrap();
        let non_eof: Vec<_> = tokens.iter().filter(|(t, _)| t.kind != TokenKind::Eof).collect();
        prop_assert_eq!(non_eof.len(), 1, "operator '{}' should produce exactly 1 token", op);
        prop_assert_ne!(non_eof[0].0.kind, TokenKind::Ident);
        prop_assert_ne!(non_eof[0].0.kind, TokenKind::Number);
    }

    #[test]
    fn prop_single_delimiter(delim in proptest::sample::select(vec![
        "(", ")", "{", "}", "[", "]", ",", ".", ":", ";",
    ])) {
        let tokens = Lexer::new(delim).scan_all().unwrap();
        let non_eof: Vec<_> = tokens.iter().filter(|(t, _)| t.kind != TokenKind::Eof).collect();
        prop_assert_eq!(non_eof.len(), 1);
        prop_assert_ne!(non_eof[0].0.kind, TokenKind::Ident);
        prop_assert_ne!(non_eof[0].0.kind, TokenKind::Number);
    }

    #[test]
    fn prop_keyword_recognition(kw in proptest::sample::select(
        nuzo_frontend::token::KEYWORDS.iter().map(|(en, _, _)| *en).collect::<Vec<_>>()
    )) {
        let tokens = Lexer::new(kw).scan_all().unwrap();
        let non_eof: Vec<_> = tokens.iter().filter(|(t, _)| t.kind != TokenKind::Eof).collect();
        prop_assert_eq!(non_eof.len(), 1);
        prop_assert_ne!(non_eof[0].0.kind, TokenKind::Ident, "keyword '{}' should not be Ident", kw);
    }

    #[test]
    fn prop_cjk_keyword_recognition(kw in proptest::sample::select(
        nuzo_frontend::token::KEYWORDS.iter().map(|(_, cn, _)| *cn).collect::<Vec<_>>()
    )) {
        let tokens = Lexer::new(kw).scan_all().unwrap();
        let non_eof: Vec<_> = tokens.iter().filter(|(t, _)| t.kind != TokenKind::Eof).collect();
        prop_assert_eq!(non_eof.len(), 1);
        prop_assert_ne!(non_eof[0].0.kind, TokenKind::Ident, "CJK keyword '{}' should not be Ident", kw);
    }

    #[test]
    fn prop_operator_sequence(
        ops in prop::collection::vec(
            proptest::sample::select(vec![
                "+", "-", "*", "/", "%", "==", "!=", "<", ">", "<=", ">=",
                "&&", "||", "=>", "+=", "-=", "*=", "/=", "??", "..", "..<", "|>",
                "(", ")", "{", "}", "[", "]", ",", ":", ";",
            ]),
            1..20
        )
    ) {
        let source = ops.join(" ");
        let tokens = Lexer::new(&source).scan_all().unwrap();
        let non_eof: Vec<_> = tokens.iter().filter(|(t, _)| t.kind != TokenKind::Eof).collect();
        prop_assert_eq!(non_eof.len(), ops.len());
    }

    #[test]
    fn prop_bom_prefix_invariant(content in "[a-zA-Z0-9 +\\-*/]{0,100}") {
        let without_bom = Lexer::new(&content).scan_all();
        let bom_source = format!("\u{FEFF}{}", content);
        let with_bom = Lexer::new(&bom_source).scan_all();
        match (&without_bom, &with_bom) {
            (Ok(t1), Ok(t2)) => {
                prop_assert_eq!(t1.len(), t2.len());
                for ((a, ta), (b, tb)) in t1.iter().zip(t2.iter()) {
                    prop_assert_eq!(a.kind, b.kind);
                    prop_assert_eq!(ta, tb);
                }
            }
            (Err(_), Err(_)) => {}
            _ => prop_assert!(false, "BOM prefix changed scan success/failure"),
        }
    }
}
