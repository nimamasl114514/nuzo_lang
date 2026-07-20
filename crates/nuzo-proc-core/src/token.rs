//! TokenStream 操作工具
//!
//! 提供 `TokenBuilder`（流式构建）、`TokenParser`（类型化提取）等高层 API。

use proc_macro2::{Delimiter, Ident, Literal, Punct, Spacing, TokenStream, TokenTree};
use std::cell::Cell;
use std::fmt;
use std::rc::Rc;
use syn::parse::{Parse, ParseStream, Parser};

// ============================================================
// TokenBuilder — 流式构建 TokenStream
// ============================================================

/// 流式 TokenStream 构建器。
///
/// 采用 Builder 模式，所有添加方法返回 `&mut Self` 以支持链式调用，
/// 最终通过 `build()` 消费自身产出 `TokenStream`。
pub struct TokenBuilder {
    tokens: Vec<TokenTree>,
}

impl TokenBuilder {
    pub fn new() -> Self {
        Self { tokens: Vec::new() }
    }

    /// 添加一个 [`Ident`] 节点。名称必须符合 Rust 标识符规范。
    pub fn ident(mut self, name: &str) -> Self {
        self.tokens.push(TokenTree::Ident(Ident::new(name, proc_macro2::Span::call_site())));
        self
    }

    /// 添加一个单字符 [`Punct`] 节点，默认使用 [`Spacing::Alone`]。
    pub fn punct(mut self, ch: char) -> Self {
        self.tokens.push(TokenTree::Punct(Punct::new(ch, Spacing::Alone)));
        self
    }

    /// 添加一个 [`Punct`] 节点并显式指定间距。
    pub fn punct_with_spacing(mut self, ch: char, spacing: Spacing) -> Self {
        self.tokens.push(TokenTree::Punct(Punct::new(ch, spacing)));
        self
    }

    /// 添加一个 [`Literal`] 节点。
    pub fn literal(mut self, lit: Literal) -> Self {
        self.tokens.push(TokenTree::Literal(lit));
        self
    }

    /// 添加一个 [`Group`] 节点，包裹内部的 TokenStream。
    pub fn group(mut self, delimiter: Delimiter, inner: TokenStream) -> Self {
        self.tokens.push(TokenTree::Group(proc_macro2::Group::new(delimiter, inner)));
        self
    }

    /// 将另一个 [`TokenStream`] 的全部 token 追加到当前流末尾。
    pub fn token_stream(mut self, ts: TokenStream) -> Self {
        self.tokens.extend(ts);
        self
    }

    /// 消费构建器，产出最终的 [`TokenStream`]。
    pub fn build(self) -> TokenStream {
        self.tokens.into_iter().collect()
    }

    /// 当前已累积的 token 数量（调试/测试用）。
    #[inline]
    pub fn len(&self) -> usize {
        self.tokens.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.tokens.is_empty()
    }
}

impl Default for TokenBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for TokenBuilder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TokenBuilder")
            .field("tokens", &self.tokens.iter().map(|t| t.to_string()).collect::<Vec<_>>())
            .finish()
    }
}

// ============================================================
// TokenParser — 从 TokenStream 中按类型提取 token
// ============================================================

/// 类型化 TokenStream 解析器。
///
/// 基于 `Vec<TokenTree>` + 游标位置的前瞻解析器，无生命周期参数，
/// 提供对 Ident / Punct / Literal 的安全前瞻与消费操作。
/// 所有消费操作在失败时返回带 span 信息的 `syn::Error`。
///
/// # 设计选择
///
/// 不直接封装 `syn::parse::ParseBuffer`，原因：
/// - `ParseBuffer` 无法从 `TokenStream` 直接构造并存储在结构体中
///   （`syn::parse2` 要求 `T: Parse`，而 `ParseBuffer` 未实现 `Parse`）
/// - `ParseBuffer::cursor()` 为 `pub(crate)`，外部无法访问
/// - `Peek` trait 对 `Ident`/`Punct`/`Literal` 的支持需要特殊处理
pub struct TokenParser {
    tokens: Vec<TokenTree>,
    pos: usize,
}

impl TokenParser {
    /// 从 [`TokenStream`] 创建解析器。
    pub fn new(input: TokenStream) -> Self {
        Self { tokens: input.into_iter().collect(), pos: 0 }
    }

    /// 剩余流是否为空。
    pub fn is_empty(&self) -> bool {
        self.pos >= self.tokens.len()
    }

    /// 前瞻：下一个 token 是否为 Ident。
    pub fn peek_ident(&self) -> bool {
        matches!(self.current(), Some(TokenTree::Ident(_)))
    }

    /// 前瞻：下一个 token 是否为指定字符的 Punct。
    pub fn peek_punct(&self, ch: char) -> bool {
        matches!(self.current(), Some(TokenTree::Punct(p)) if p.as_char() == ch)
    }

    /// 前瞻：下一个 token 是否为 Literal。
    pub fn peek_literal(&self) -> bool {
        matches!(self.current(), Some(TokenTree::Literal(_)))
    }

    /// 消费并返回下一个 [`Ident`]。
    ///
    /// # Errors
    /// 当下一个 token 不是 Ident 时返回 `syn::Error`。
    pub fn expect_ident(&mut self) -> syn::Result<Ident> {
        match self.advance() {
            Some(TokenTree::Ident(id)) => Ok(id),
            Some(other) => Err(syn::Error::new(other.span(), "expected identifier")),
            None => Err(syn::Error::new(
                proc_macro2::Span::call_site(),
                "unexpected end of input, expected identifier",
            )),
        }
    }

    /// 消费并返回指定字符的 [`Punct`]。
    ///
    /// # Errors
    /// 当下一个 token 不是目标 Punct 时返回 `syn::Error`。
    pub fn expect_punct(&mut self, ch: char) -> syn::Result<Punct> {
        match self.advance() {
            Some(TokenTree::Punct(p)) if p.as_char() == ch => Ok(p),
            Some(TokenTree::Punct(p)) => {
                Err(syn::Error::new(p.span(), format!("expected '{}', got '{}'", ch, p.as_char())))
            }
            Some(other) => Err(syn::Error::new(other.span(), format!("expected '{}'", ch))),
            None => Err(syn::Error::new(
                proc_macro2::Span::call_site(),
                format!("unexpected end of input, expected '{}'", ch),
            )),
        }
    }

    /// 消费并返回下一个 [`Literal`]。
    ///
    /// # Errors
    /// 当下一个 token 不是 Literal 时返回 `syn::Error`。
    pub fn expect_literal(&mut self) -> syn::Result<Literal> {
        match self.advance() {
            Some(TokenTree::Literal(lit)) => Ok(lit),
            Some(other) => Err(syn::Error::new(other.span(), "expected literal")),
            None => Err(syn::Error::new(
                proc_macro2::Span::call_site(),
                "unexpected end of input, expected literal",
            )),
        }
    }

    /// 使用 `syn::parse::Parse` trait 解析任意类型 T。
    ///
    /// 内部将剩余 token 收集为 `TokenStream`，通过 syn 的 `Parser` 机制解析，
    /// 解析成功后自动将游标推进到未消费的位置。
    ///
    /// # Errors
    /// 当 `T::parse` 失败时返回 `syn::Error`。
    pub fn parse<T: Parse>(&mut self) -> syn::Result<T> {
        let remaining: TokenStream = self.remaining();

        // 使用 Rc<Cell<>> 在 FnOnce 闭包内外传递 leftover，
        // 因为 FnOnce 消耗捕获变量，无法通过返回值传出。
        let leftover: Rc<Cell<Option<TokenStream>>> = Rc::new(Cell::new(None));
        let leftover_clone = leftover.clone();

        let result: T = (move |stream: ParseStream<'_>| -> syn::Result<T> {
            let value: T = stream.parse()?;
            leftover_clone.set(Some(stream.parse::<TokenStream>()?));
            Ok(value)
        })
        .parse2(remaining)?;

        self.tokens = leftover.take().unwrap_or_default().into_iter().collect();
        self.pos = 0;
        Ok(result)
    }

    /// 返回剩余的 token 流（不消费，克隆返回）。
    pub fn remaining(&self) -> TokenStream {
        self.tokens[self.pos..].iter().cloned().collect()
    }

    // ── 内部辅助 ───────────────────────────────────────────

    /// 获取当前位置的 token 引用（前瞻，不消费）。
    fn current(&self) -> Option<&TokenTree> {
        self.tokens.get(self.pos)
    }

    /// 前进一位，返回被消费的 token（克隆）。
    fn advance(&mut self) -> Option<TokenTree> {
        if self.pos < self.tokens.len() {
            let token = self.tokens[self.pos].clone();
            self.pos += 1;
            Some(token)
        } else {
            None
        }
    }
}

impl fmt::Debug for TokenParser {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TokenParser")
            .field("pos", &self.pos)
            .field("total", &self.tokens.len())
            .field(
                "remaining",
                &self.tokens[self.pos..].iter().map(|t| t.to_string()).collect::<Vec<_>>(),
            )
            .finish()
    }
}

// ============================================================
// 便捷函数
// ============================================================

/// 前瞻 + 条件消费：若下一个 token 为指定的标点符号则消费它，否则不动。
///
/// 直接操作 `ParseStream`，通过 `fork` 实现无副作用前瞻，
/// 确认匹配后才从原始流消费。
///
/// Returns `true` iff the punctuation was consumed.
pub fn peek_and_consume(input: ParseStream<'_>, punct: char) -> bool {
    // fork 创建独立副本，parse 不影响原始流
    let fork = input.fork();
    let matches = fork.parse::<Punct>().is_ok_and(|p| p.as_char() == punct);
    if matches {
        let _ = input.parse::<Punct>();
    }
    matches
}

/// 解析逗号分隔的零或多个 T 元素。
///
/// 支持 trailing comma（尾随逗号），空输入返回空列表。
pub fn parse_comma_separated<T: Parse>(input: ParseStream<'_>) -> syn::Result<Vec<T>> {
    let mut items = Vec::new();

    if input.is_empty() {
        return Ok(items);
    }

    loop {
        items.push(input.parse()?);

        if !peek_and_consume(input, ',') {
            break;
        }

        if input.is_empty() {
            break;
        }
    }

    Ok(items)
}

// ============================================================
// Tests
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use quote::quote;

    // ── TokenBuilder 测试 ────────────────────────────────────

    mod token_builder_tests {
        use super::*;

        #[test]
        fn empty_builder_produces_empty_stream() {
            let ts = TokenBuilder::new().build();
            assert!(ts.is_empty());
        }

        #[test]
        fn single_ident() {
            let ts = TokenBuilder::new().ident("hello").build();
            assert_eq!(ts.to_string(), "hello");
        }

        #[test]
        fn single_punct() {
            let ts = TokenBuilder::new().punct('+').build();
            assert_eq!(ts.to_string(), "+");
        }

        #[test]
        fn single_literal() {
            let ts = TokenBuilder::new().literal(Literal::i64_suffixed(42)).build();
            assert_eq!(ts.to_string(), "42i64");
        }

        #[test]
        fn chain_multiple_tokens() {
            // 注意：'(' 和 ')' 是 delimiter 而非 Punct，不能通过 punct() 添加，
            // 必须使用 group() 包裹。
            let body = quote! { println!("hi"); };
            let ts = TokenBuilder::new()
                .ident("fn")
                .ident("main")
                .group(Delimiter::Parenthesis, TokenStream::new())
                .group(Delimiter::Brace, body)
                .build();
            let expected = "fn main () { println ! (\"hi\") ; }";
            assert_eq!(ts.to_string(), expected);
        }

        #[test]
        fn merge_token_streams() {
            let a: TokenStream = quote! { a + b };
            let b: TokenStream = quote! { * c };
            let ts = TokenBuilder::new().token_stream(a).punct('+').token_stream(b).build();
            assert_eq!(ts.to_string(), "a + b + * c");
        }

        #[test]
        fn nested_group() {
            let inner = TokenBuilder::new().ident("x").punct(',').ident("y").build();
            let ts = TokenBuilder::new()
                .ident("vec")
                .punct('!')
                .group(Delimiter::Bracket, inner)
                .build();
            assert_eq!(ts.to_string(), "vec ! [x , y]");
        }

        #[test]
        fn punct_with_joint_spacing() {
            let ts = TokenBuilder::new().punct_with_spacing(':', Spacing::Joint).punct(':').build();
            assert_eq!(ts.to_string(), "::");
        }

        #[test]
        fn len_and_is_empty() {
            let b = TokenBuilder::new();
            assert_eq!(b.len(), 0);
            assert!(b.is_empty());
            let b = b.ident("x");
            assert_eq!(b.len(), 1);
            assert!(!b.is_empty());
        }

        #[test]
        fn debug_format_shows_tokens() {
            let b = TokenBuilder::new().ident("foo").punct('+').literal(Literal::i64_unsuffixed(1));
            let debug_str = format!("{:?}", b);
            assert!(debug_str.contains("TokenBuilder"));
            assert!(debug_str.contains("foo"));
        }
    }

    // ── TokenParser 测试 ─────────────────────────────────────

    mod token_parser_tests {
        use super::*;

        #[test]
        fn from_valid_stream() {
            let ts = quote! { hello world };
            let parser = TokenParser::new(ts);
            assert!(!parser.is_empty());
        }

        #[test]
        fn is_empty_detection() {
            let ts = TokenStream::new();
            let parser = TokenParser::new(ts);
            assert!(parser.is_empty());
        }

        #[test]
        fn peek_ident_true() {
            let ts = quote! { foo bar };
            let parser = TokenParser::new(ts);
            assert!(parser.peek_ident());
        }

        #[test]
        fn peek_punct_match() {
            let ts = quote! { , rest };
            let parser = TokenParser::new(ts);
            assert!(parser.peek_punct(','));
            assert!(!parser.peek_punct(';'));
        }

        #[test]
        fn peek_literal_true() {
            let ts = quote! { 42 };
            let parser = TokenParser::new(ts);
            assert!(parser.peek_literal());
        }

        #[test]
        fn expect_ident_success() {
            let ts = quote! { my_ident };
            let mut parser = TokenParser::new(ts);
            let id = parser.expect_ident().unwrap();
            assert_eq!(id.to_string(), "my_ident");
        }

        #[test]
        fn expect_ident_failure_on_punct() {
            let ts = quote! { + };
            let mut parser = TokenParser::new(ts);
            assert!(parser.expect_ident().is_err());
        }

        #[test]
        fn expect_literal_success() {
            let ts = quote! { "hello" };
            let mut parser = TokenParser::new(ts);
            let lit = parser.expect_literal().unwrap();
            assert_eq!(lit.to_string(), "\"hello\"");
        }

        #[test]
        fn expect_literal_failure_on_ident() {
            let ts = quote! { not_a_literal };
            let mut parser = TokenParser::new(ts);
            assert!(parser.expect_literal().is_err());
        }

        #[test]
        fn expect_punct_success() {
            let ts = quote! { : };
            let mut parser = TokenParser::new(ts);
            let p = parser.expect_punct(':').unwrap();
            assert_eq!(p.as_char(), ':');
        }

        #[test]
        fn expect_punct_wrong_char() {
            let ts = quote! { ; };
            let mut parser = TokenParser::new(ts);
            let result = parser.expect_punct(':');
            assert!(result.is_err());
        }

        #[test]
        fn parse_generic_type() {
            let ts = quote! { Option<String> };
            let mut parser = TokenParser::new(ts);
            let ty: syn::Type = parser.parse().unwrap();
            assert_eq!(quote!(#ty).to_string(), "Option < String >");
        }

        #[test]
        fn sequential_consumption() {
            let ts = quote! { foo + 42 };
            let mut parser = TokenParser::new(ts);

            assert_eq!(parser.expect_ident().unwrap().to_string(), "foo");
            assert_eq!(parser.expect_punct('+').unwrap().as_char(), '+');
            assert_eq!(parser.expect_literal().unwrap().to_string(), "42");
            assert!(parser.is_empty());
        }

        #[test]
        fn poison_pill_empty_expect() {
            let ts = TokenStream::new();
            let mut parser = TokenParser::new(ts);
            assert!(parser.expect_ident().is_err());
            assert!(parser.expect_punct(',').is_err());
            assert!(parser.expect_literal().is_err());
        }

        #[test]
        fn remaining_returns_unconsumed() {
            let ts = quote! { a b c };
            let mut parser = TokenParser::new(ts);
            let _ = parser.expect_ident().unwrap();
            let rest = parser.remaining();
            assert_eq!(rest.to_string(), "b c");
        }

        #[test]
        fn debug_format_shows_position() {
            let ts = quote! { a b c };
            let mut parser = TokenParser::new(ts);
            let _ = parser.expect_ident().unwrap();
            let debug_str = format!("{:?}", parser);
            assert!(debug_str.contains("TokenParser"));
            assert!(debug_str.contains("pos"));
        }
    }

    // ── 便捷函数测试 ────────────────────────────────────────

    mod utility_tests {
        use super::*;

        #[test]
        fn peek_and_consume_matches() {
            let ts = quote! { , rest };
            let result = (|input: ParseStream<'_>| -> syn::Result<bool> {
                let result = peek_and_consume(input, ',');
                let _ = input.parse::<TokenStream>(); // drain remaining
                Ok(result)
            })
            .parse2(ts)
            .unwrap();
            assert!(result);
        }

        #[test]
        fn peek_and_consume_no_match() {
            let ts = quote! { ; rest };
            let result = (|input: ParseStream<'_>| -> syn::Result<bool> {
                let result = peek_and_consume(input, ',');
                let _ = input.parse::<TokenStream>(); // drain remaining
                Ok(result)
            })
            .parse2(ts)
            .unwrap();
            assert!(!result);
        }

        #[test]
        fn peek_and_consume_actually_consumes() {
            let ts = quote! { , rest };
            let result = (|input: ParseStream<'_>| -> syn::Result<bool> {
                let consumed = peek_and_consume(input, ',');
                // 验证逗号确实被消费了：下一个 token 应该是 "rest"
                let next: Option<Ident> = input.parse().ok();
                assert_eq!(next.map(|i| i.to_string()), Some("rest".to_string()));
                let _ = input.parse::<TokenStream>(); // drain remaining
                Ok(consumed)
            })
            .parse2(ts)
            .unwrap();
            assert!(result);
        }

        #[test]
        fn parse_comma_separated_basic() {
            let ts = quote! { a, b, c };
            let items: Vec<Ident> =
                (|input: ParseStream<'_>| parse_comma_separated(input)).parse2(ts).unwrap();
            let names: Vec<String> = items.iter().map(|i| i.to_string()).collect();
            assert_eq!(names, vec!["a", "b", "c"]);
        }

        #[test]
        fn parse_comma_separated_single() {
            let ts = quote! { solo };
            let items: Vec<Ident> =
                (|input: ParseStream<'_>| parse_comma_separated(input)).parse2(ts).unwrap();
            assert_eq!(items.len(), 1);
            assert_eq!(items[0].to_string(), "solo");
        }

        #[test]
        fn parse_comma_separated_trailing_comma() {
            let ts = quote! { x, y, };
            let items: Vec<Ident> =
                (|input: ParseStream<'_>| parse_comma_separated(input)).parse2(ts).unwrap();
            assert_eq!(items.len(), 2);
        }

        #[test]
        fn parse_comma_separated_empty() {
            let ts = TokenStream::new();
            let items: Vec<Ident> =
                (|input: ParseStream<'_>| parse_comma_separated(input)).parse2(ts).unwrap();
            assert!(items.is_empty());
        }

        #[test]
        fn roundtrip_build_then_parse() {
            let built = TokenBuilder::new()
                .ident("let")
                .ident("x")
                .punct('=')
                .literal(Literal::i64_unsuffixed(10))
                .punct(';')
                .build();

            let mut parser = TokenParser::new(built);
            assert_eq!(parser.expect_ident().unwrap().to_string(), "let");
            assert_eq!(parser.expect_ident().unwrap().to_string(), "x");
            assert_eq!(parser.expect_punct('=').unwrap().as_char(), '=');
            assert_eq!(parser.expect_literal().unwrap().to_string(), "10");
            assert_eq!(parser.expect_punct(';').unwrap().as_char(), ';');
            assert!(parser.is_empty());
        }
    }
}
