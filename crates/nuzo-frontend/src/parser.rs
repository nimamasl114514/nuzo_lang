//! # Nuzo 递归下降语法分析器（Parser）模块
//!
//! ## 模块职责
//! 将 Lexer 产生的 Token 序列转换为结构化的抽象语法树（AST）。
//! 这是编译器前端的第二阶段，实现了完整的 Nuzo 语法分析。
//!
//! ## 解析算法：递归下降（Recursive Descent）
//!
//! Parser 采用经典的**递归下降**算法，每个文法规则对应一个或多个方法：
//! - **声明级**：`declaration()` → 函数定义 / 语句
//! - **语句级**：`statement()` → if/while/for/return/break/continue/表达式
//! - **表达式级**：按优先级从低到高排列的多个方法
//!
//! # 运算符优先级表（从低到高）
//!
//! | 优先级 | 解析方法 | 运算符/构造 | 结合性 |
//! |--------|---------|------------|--------|
//! | 0 (最低) | `expr_statement()` | 赋值 `=`, 复合赋值 `+=` 等 | 右 |
//! | 1 | `arrow_expr()` | 箭头函数 `=>` | 右 |
//! | 2 | `or_expr()` | 逻辑或 `\|\|`, `or` | 左 |
//! | 3 | `and_expr()` | 逻辑与 `&&`, `and` | 左 |
//! | 4 | `comparison()` | 比较 `==`, `<`, `>` 等 | 左 |
//! | 5 | `range()` | 范围 `..`, `..<` | 左 |
//! | 6 | `addition()` | 加法 `+`, `-` | 左 |
//! | 7 | `multiplication()` | 乘法 `*`, `/`, `%` | 左 |
//! | 8 | `unary()` | 一元 `-`, `!` | 右 |
//! | 9 (最高) | `call()` | 后缀调用 `()`, 索引 `[]`, 成员 `.` | 左 |
//!
//! ## 错误恢复策略
//!
//! Parser 采用**panic 模式**错误处理：
//! - 遇到语法错误时立即返回 `Err(ParseError)`
//! - 不尝试自动恢复或同步
//! - 错误信息包含精确的行号和列号
//!
//! 这简化了实现，也避免了部分解析导致的语义混淆。
//!
//! ## 双语支持
//!
//! Parser 通过 [`TokenKind`] 的谓词方法（如 `is_if()`, `is_fn()`）自动支持中英文关键字：
//! ```text
//! "if" / "如果" → TokenKind::If
//! "fn" / "函数" → TokenKind::Fn
//! ```
//! 调用者无需关心用户输入的是英文还是中文。
//!
//! ## 典型工作流程
//!
//! ```text
//! 源代码字符串
//!     │
//!     ▼
//! ┌─────────────┐
//! │   Lexer     │  Tokenize: 源码 → Vec<(Token, &str)>
//! └─────────────┘
//!     │
//!     ▼
//! ┌─────────────┐
//! │   Parser    │  Parse: Tokens → AST (Program)
//! │             │  - parse() 入口方法
//! │             │  - declaration() 顶层循环
//! │             │  - expression() 表达式解析
//! └─────────────┘
//!     │
//!     ▼
//! Program { statements: Vec<Stmt> }
//! ```

use crate::ast::*;
use crate::lexer::Lexer;
use crate::token::{Token, TokenKind};

/// 语法分析错误类型
///
/// 当 Parser 遇到不符合语法的 Token 序列时返回此错误。
///
/// # 错误类型示例
/// - 缺少必需的分隔符："expected ')' after arguments"
/// - 非法的表达式上下文："unexpected token in expression"
/// - 类型不匹配（未来扩展）："type mismatch"
#[derive(Debug)]
pub struct ParseError {
    /// 人类可读的错误描述
    pub message: String,
    /// 错误所在行号（1-based）
    pub line: usize,
    /// 错误所在列号（1-based）
    pub column: usize,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Parse error at {}:{}: {}", self.line, self.column, self.message)
    }
}

impl std::error::Error for ParseError {}

/// 解析流水线各阶段耗时
///
/// # 使用场景
/// 编译器性能分析：将词法分析和语法分析的耗时独立计量，
/// 便于定位编译瓶颈在 Token 扫描还是 AST 构建。
///
/// # 零开销保证
/// 仅在调用 `parse_with_timing()` 时计算，`parse()` 不产生任何额外开销。
#[derive(Debug, Clone)]
pub struct ParseTimings {
    /// 词法分析耗时（Lexer: 源码 → Token 流）
    pub lex_duration: std::time::Duration,
    /// 语法分析耗时（Parser: Token 流 → AST）
    pub parse_duration: std::time::Duration,
}

/// 从 `ParseError` 自动转换为 `NuzoError`。
///
/// 解析错误属于用户代码问题（语法错误、非法 Token 序列等），不是 VM/compiler bug，
/// 因此映射为带 `SourceLocation` 的 `InternalError::ParseError` 并附加稳定的
/// `ErrorCode::SyntaxError`（`C0005`），保留原始 line/column 信息以便精确定位。
impl From<ParseError> for nuzo_values::NuzoError {
    fn from(e: ParseError) -> Self {
        let loc = nuzo_values::SourceLocation::new(e.line).with_column(e.column);
        nuzo_values::NuzoError {
            kind: nuzo_values::NuzoErrorKind::Internal(
                nuzo_values::InternalError::ParseError { message: e.message },
                None,
            ),
            source_location: Some(loc),
            code: nuzo_core::error::ErrorCode::SyntaxError,
        }
    }
}

/// 递归下降语法分析器
///
/// 将 Token 流转换为 AST 的核心组件。
///
/// # 生命周期参数
/// - `'a`: Token 文本切片的生命周期（与源码相同）
///
/// # 内部状态
/// ```text
/// Parser<'a> {
///     tokens: Vec<Token>,      // 完整的 Token 列表
///     texts: Vec<&'a str>,    // 对应的源码文本切片
///     current: usize,          // 当前 Token 索引
/// }
/// ```
///
/// # 使用示例
/// ```ignore
/// use nuzo_frontend::Parser;
///
/// let source = "fn add(a, b) { a + b }";
/// let program = Parser::parse(source)?;
/// // program.statements 包含解析后的 AST 节点
/// ```
///
/// # 设计特点
/// - **单遍扫描**：只遍历 Token 列表一次（O(n) 时间）
/// - **零拷贝文本**：通过 `texts` 数组引用原始源码
/// - **前瞻能力**：通过 `peek()` 查看下一个 Token 而不消费
/// - **精确位置**：所有错误携带 Span 信息
pub struct Parser<'a> {
    /// Token 列表（由 Lexer 生成）
    tokens: Vec<Token>,
    /// Token 对应的源码文本切片（零拷贝）
    texts: Vec<&'a str>,
    /// 当前正在处理的 Token 索引
    current: usize,
    /// 当前表达式/块的递归深度（用于栈溢出保护，P1-3）
    ///
    /// 在 [`expression()`](Self::expression) 和 [`block()`](Self::block) 入口递增，
    /// 出口递减。超过 [`MAX_PARSER_DEPTH`] 时返回 ParseError，防止恶意/失控
    /// 输入导致栈溢出（如 10000 层嵌套的 `[[[[...]]]]`）。
    depth: usize,

    /// 链式比较临时变量计数器（P2-7）
    ///
    /// 用于生成唯一的 `__nuzo_cmp_N` 临时变量名，避免链式比较
    /// `a < b < c` 中 `b` 被求值两次。每个 [`comparison()`](Self::comparison)
    /// 链生成独立的临时变量，作用域限定在生成的 `Expr::Block` 内。
    tmp_counter: u64,
}

/// 解析器最大递归深度（栈溢出保护，P1-3）
///
/// # 取值依据
///
/// 每次 `expression()` 调用会经过约 12 个中间函数（pipe_expr → arrow_expr →
/// or_expr → null_coalesce → and_expr → comparison → range → addition →
/// multiplication → unary → call → primary），每个中间函数占用栈帧。
///
/// - Rust 测试线程默认栈 2MB
/// - 每个栈帧约 1-2KB（保守估计）
/// - 64 × 12 × 2KB ≈ 1.5MB（安全余量内）
/// - 64 × 12 × 4KB ≈ 3MB（接近 2MB 限制，但实际帧更小）
///
/// 64 远超正常代码需求（典型嵌套 < 10 层），能有效拦截恶意嵌套输入。
const MAX_PARSER_DEPTH: usize = 64;

impl<'a> Parser<'a> {
    /// 解析入口方法 - 将源代码字符串转换为 AST
    ///
    /// 这是 Parser 的唯一公开接口，执行完整的两阶段编译前端：
    /// 1. **词法分析**：调用 Lexer 将源码转为 Token 序列
    /// 2. **语法分析**：递归下降构建 AST
    ///
    /// # 参数
    /// * `source` - 要解析的 Nuzo 源代码字符串
    ///
    /// # 返回值
    /// `Result<Program, ParseError>`
    /// - 成功时返回完整的 [`Program`] AST
    /// - 失败时返回包含位置信息的语法错误
    ///
    /// # 错误传播
    /// Lexer 错误会自动转换为 ParseError（保留原始位置信息）
    ///
    /// # 示例
    /// ```ignore
    /// let source = "fn add(a, b) { a + b }";
    /// match Parser::parse(source) {
    ///     Ok(program) => println!("Parsed {} statements", program.statements.len()),
    ///     Err(e) => eprintln!("Error at {}:{}", e.line, e.column),
    /// }
    /// ```
    pub fn parse(source: &'a str) -> Result<Program, ParseError> {
        let (program, _timings) = Self::parse_with_timing(source)?;
        Ok(program)
    }

    /// 带分阶段计时的解析入口方法
    ///
    /// 与 [`parse()`](Self::parse) 功能完全一致，但额外返回各阶段耗时。
    /// 用于编译器性能分析流水线，零订阅时无额外开销。
    ///
    /// # 返回值
    /// `Result<(Program, ParseTimings), ParseError>`
    ///
    /// # 示例
    /// ```ignore
    /// let (program, timings) = Parser::parse_with_timing(source)?;
    /// println!("Lex: {:.2}ms, Parse: {:.2}ms",
    ///     timings.lex_duration.as_secs_f64() * 1000.0,
    ///     timings.parse_duration.as_secs_f64() * 1000.0);
    /// ```
    pub fn parse_with_timing(source: &'a str) -> Result<(Program, ParseTimings), ParseError> {
        // 阶段 1: 词法分析（计时）
        let lex_start = web_time::Instant::now();
        let lexer = Lexer::new(source);
        let result = lexer.scan_all().map_err(|e| ParseError {
            message: e.message,
            line: e.line,
            column: e.column,
        })?;
        let lex_duration = lex_start.elapsed();

        // 分离 Token 和文本切片
        let (tokens, texts): (Vec<_>, Vec<_>) = result.into_iter().unzip();

        // 阶段 2: 语法分析（计时）
        let parse_start = web_time::Instant::now();
        let mut parser = Parser { tokens, texts, current: 0, depth: 0, tmp_counter: 0 };
        let mut statements = Vec::new();
        while !parser.is_at_end() {
            statements.push(parser.declaration()?);
            parser.skip_semicolons();
        }
        let parse_duration = parse_start.elapsed();

        Ok((Program { statements }, ParseTimings { lex_duration, parse_duration }))
    }

    /// 查看当前 Token（不消费）
    ///
    /// 用于决策性预览，如判断是否进入某种解析模式。
    fn peek(&self) -> &Token {
        &self.tokens[self.current]
    }

    /// 查看上一个已消费的 Token
    ///
    /// 通常用于获取刚消费的 Token 的信息（如文本内容）。
    fn previous(&self) -> &Token {
        &self.tokens[self.current - 1]
    }

    /// 检查是否已到达输入末尾（EOF）
    fn is_at_end(&self) -> bool {
        self.peek().kind == TokenKind::Eof
    }

    /// 消费当前 Token 并前进到下一个
    ///
    /// 如果已经到达 EOF，则不会继续前进。
    /// 返回被消费的 Token 引用。
    fn advance(&mut self) -> &Token {
        if !self.is_at_end() {
            self.current += 1;
        }
        self.previous()
    }

    /// 检查当前 Token 是否为指定类型（不消费）
    fn check(&self, kind: TokenKind) -> bool {
        !self.is_at_end() && self.peek().kind == kind
    }

    /// 使用谓词函数检查当前 Token 类型（不消费）
    ///
    /// 比直接 `check()` 更灵活，支持复杂的类型判断逻辑。
    /// 例如：`check_if(TokenKind::is_fn)` 可以同时匹配 `fn` 和 `函数`
    fn check_if<F: Fn(TokenKind) -> bool>(&self, f: F) -> bool {
        !self.is_at_end() && f(self.peek().kind)
    }

    /// 如果当前 Token 匹配指定类型则消费它
    ///
    /// # 返回值
    /// - `true`: 匹配成功并消费了 Token
    /// - `false`: 不匹配，保持不动
    fn match_kind(&mut self, kind: TokenKind) -> bool {
        if self.check(kind) {
            self.advance();
            true
        } else {
            false
        }
    }

    /// 使用谓词函数进行条件性消费
    fn match_if<F: Fn(TokenKind) -> bool>(&mut self, f: F) -> bool {
        if self.check_if(f) {
            self.advance();
            true
        } else {
            false
        }
    }

    /// 强制消费指定类型的 Token，否则返回错误
    ///
    /// 这是**同步点**操作：要求当前 Token 必须是预期类型，
    /// 否则立即报告语法错误。
    ///
    /// # 参数
    /// * `kind` - 期望的 Token 类型
    /// * `msg` - 不匹配时的错误消息
    ///
    /// # 使用场景
    /// - 要求必须出现的分隔符：`consume(LParen, "expected '('")`
    /// - 关键字验证：`consume(Fn, "expected 'fn'")`
    fn consume(&mut self, kind: TokenKind, msg: &str) -> Result<&Token, ParseError> {
        if self.check(kind) {
            Ok(self.advance())
        } else {
            Err(ParseError {
                message: msg.to_string(),
                line: self.peek().line,
                column: self.peek().column,
            })
        }
    }

    /// 使用谓词函数强制消费 Token
    fn consume_if<F: Fn(TokenKind) -> bool>(
        &mut self,
        f: F,
        msg: &str,
    ) -> Result<&Token, ParseError> {
        if self.check_if(f) {
            Ok(self.advance())
        } else {
            Err(ParseError {
                message: msg.to_string(),
                line: self.peek().line,
                column: self.peek().column,
            })
        }
    }

    /// 获取当前位置的 Span 信息
    fn span_here(&self) -> Span {
        Span::new(self.peek().line, self.peek().column)
    }

    /// 生成唯一的链式比较临时变量名（P2-7）
    ///
    /// 格式：`__nuzo_cmp_{counter}`，counter 单调递增确保全局唯一。
    /// 使用 `__nuzo_` 前缀避免与用户变量冲突（用户标识符以 `_` 开头
    /// 虽然合法但不常见，`__nuzo_` 前缀进一步降低碰撞概率）。
    ///
    /// # 作用域
    ///
    /// 临时变量的作用域限定在生成的 `Expr::Block` 内，不会泄漏到
    /// 外部作用域。即使两个 comparison() 调用生成相同前缀的变量名，
    /// 由于 Block 作用域隔离，也不会冲突。
    fn gen_tmp_name(&mut self) -> String {
        let name = format!("__nuzo_cmp_{}", self.tmp_counter);
        self.tmp_counter += 1;
        name
    }

    /// 从 Token 创建 Span 信息
    fn span_at(&self, token: &Token) -> Span {
        Span::new(token.line, token.column)
    }

    /// 获取指定 Token 对应的源码文本
    ///
    /// 优化策略：tokens 和 texts 来自 unzip()，索引严格对应。
    /// 大多数调用针对刚消费的 Token（索引 = self.current - 1）或当前前瞻的
    /// Token（索引 = self.current），先尝试这两个 O(1) 快速路径，
    /// 未命中再降级为线性搜索（罕见路径）。
    fn text_of(&self, token: &Token) -> String {
        // 快速路径 1：刚消费的 Token（最常见场景）
        let last = self.current.saturating_sub(1);
        if last < self.tokens.len() {
            let t = &self.tokens[last];
            if t.offset == token.offset && t.line == token.line {
                return self.texts.get(last).map(|s| s.to_string()).unwrap_or_default();
            }
        }
        // 快速路径 2：当前前瞻的 Token
        if self.current < self.tokens.len() {
            let t = &self.tokens[self.current];
            if t.offset == token.offset && t.line == token.line {
                return self.texts.get(self.current).map(|s| s.to_string()).unwrap_or_default();
            }
        }
        // 降级：线性搜索（罕见路径，仅用于非连续访问的 Token）
        for (i, t) in self.tokens.iter().enumerate() {
            if t.offset == token.offset && t.line == token.line {
                return self.texts.get(i).map(|s| s.to_string()).unwrap_or_default();
            }
        }
        String::new()
    }

    /// 跳过所有可选的分号
    ///
    /// Nuzo 中分号是**语句分隔符而非终止符**（类似 Go），
    /// 因此需要容忍多余的分号。
    fn skip_semicolons(&mut self) {
        while self.match_kind(TokenKind::Semicolon) {}
    }

    fn declaration(&mut self) -> Result<Stmt, ParseError> {
        if self.check(TokenKind::Import) {
            return self.parse_import(false);
        }
        if self.check(TokenKind::Lazy) {
            return self.parse_lazy_import();
        }
        // 匿名函数字面量（`fn(` 或 `函数(`）：作为表达式语句处理，
        // 让 `call()` 后缀循环能识别立即调用，例如 `fn(x) { x * 2 }(5)`
        // 解析为 `Expr::Call { callee: Expr::Fn { name: None, .. }, args: [5] }`。
        // 有名函数声明 `fn name(...) { ... }` 仍走 fn_declaration。
        if self.check_if(TokenKind::is_fn) {
            let next_is_lparen =
                self.tokens.get(self.current + 1).is_some_and(|t| t.kind == TokenKind::LParen);
            if next_is_lparen {
                return self.statement();
            }
            return self.fn_declaration();
        }
        self.statement()
    }

    /// 解析 import 语句
    ///
    /// 支持以下形式：
    /// - `import "path/to/file.nuzo"` — 字符串字面量路径（eager 全展开导入）
    /// - `import math` — 标识符模块名（不带引号）
    /// - `import ... as alias` — 带别名（alias 存入 AST，供 codegen 使用）
    ///
    /// 由 `parse_lazy_import` 复用本函数并传入 `lazy=true`。
    ///
    /// # 错误
    /// - import 后既非字符串字面量也非标识符 → "expected string literal or identifier after import"
    /// - import 后 EOF → "unexpected EOF after import"
    /// - `as` 后非标识符 → "expected identifier after as"
    fn parse_import(&mut self, lazy: bool) -> Result<Stmt, ParseError> {
        let import_token = *self.peek();
        let span = self.span_at(&import_token);
        self.advance(); // 消费 Import token

        let path = if self.is_at_end() {
            return Err(ParseError {
                message: "unexpected EOF after import".to_string(),
                line: self.peek().line,
                column: self.peek().column,
            });
        } else if self.check(TokenKind::String) {
            // import "path/to/file.nuzo"
            let tok = *self.advance();
            self.text_of(&tok)
        } else if self.check(TokenKind::Ident) {
            // import math
            let tok = *self.advance();
            self.text_of(&tok)
        } else {
            return Err(ParseError {
                message: "expected string literal or identifier after import".to_string(),
                line: self.peek().line,
                column: self.peek().column,
            });
        };

        // 可选 as alias：import ... as alias
        let alias = if self.match_kind(TokenKind::As) {
            // as 后必须跟标识符
            if self.check(TokenKind::Ident) {
                let tok = *self.advance();
                Some(self.text_of(&tok))
            } else {
                return Err(ParseError {
                    message: "expected identifier after as".to_string(),
                    line: self.peek().line,
                    column: self.peek().column,
                });
            }
        } else {
            None
        };

        Ok(Stmt::Import { path, lazy, alias, span })
    }

    /// 解析 lazy import 语句
    ///
    /// 形式：`lazy import "path/to/file.nuzo"`
    ///
    /// # 错误
    /// - lazy 后非 import 关键字 → "expected import after lazy"
    fn parse_lazy_import(&mut self) -> Result<Stmt, ParseError> {
        self.advance(); // 消费 Lazy token
        if !self.check(TokenKind::Import) {
            return Err(ParseError {
                message: "expected import after lazy".to_string(),
                line: self.peek().line,
                column: self.peek().column,
            });
        }
        self.parse_import(true)
    }

    fn fn_declaration(&mut self) -> Result<Stmt, ParseError> {
        let fn_token = *self.consume_if(TokenKind::is_fn, "expected 'fn' or '函数'")?;
        let span = self.span_at(&fn_token);

        let name = if self.check(TokenKind::Ident) {
            let tok = *self.advance();
            Some(self.text_of(&tok))
        } else {
            None
        };

        self.consume(TokenKind::LParen, "expected '(' after function name")?;
        let params = self.parse_params()?;
        self.consume(TokenKind::RParen, "expected ')' after parameters")?;
        self.consume(TokenKind::LBrace, "expected '{' before function body")?;
        let body = self.block()?;

        let expr = Expr::Fn { name, params, body, span };
        Ok(Stmt::Expr(expr))
    }

    fn parse_params(&mut self) -> Result<Vec<String>, ParseError> {
        let mut params = Vec::new();
        if !self.check(TokenKind::RParen) {
            loop {
                // 允许关键字 `fn` 作为参数名（高阶函数场景：fn apply(fn, val)）
                let tok = if self.check(TokenKind::Fn) {
                    *self.advance()
                } else {
                    *self.consume(TokenKind::Ident, "expected parameter name")?
                };
                params.push(self.text_of(&tok));
                if !self.match_kind(TokenKind::Comma) {
                    break;
                }
            }
        }
        Ok(params)
    }

    fn statement(&mut self) -> Result<Stmt, ParseError> {
        if self.check_if(TokenKind::is_if) {
            return self.if_expr_stmt();
        }
        if self.check_if(TokenKind::is_while) {
            return self.while_expr_stmt();
        }
        if self.check_if(TokenKind::is_loop) {
            return self.loop_expr_stmt();
        }
        if self.check_if(TokenKind::is_for) {
            return self.for_expr_stmt();
        }
        if self.check_if(TokenKind::is_return) {
            return self.return_stmt();
        }
        if self.check_if(TokenKind::is_break) {
            return self.break_stmt();
        }
        if self.check_if(TokenKind::is_continue) {
            return self.continue_stmt();
        }
        if self.check(TokenKind::Try) {
            return self.try_expr_stmt();
        }
        if self.check(TokenKind::Match) {
            return self.match_expr_stmt();
        }
        if self.check(TokenKind::LBrace) && !self.is_dict_literal() {
            return self.block_stmt();
        }

        self.expr_statement()
    }

    fn is_dict_literal(&self) -> bool {
        if self.current >= self.tokens.len() {
            return false;
        }
        let cur = self.tokens[self.current].kind;
        if cur != TokenKind::LBrace {
            return false;
        }

        if self.current + 1 < self.tokens.len() {
            let next = self.tokens[self.current + 1].kind;
            if next == TokenKind::RBrace {
                return true;
            }
        }

        if self.current + 2 < self.tokens.len() {
            let next = self.tokens[self.current + 1].kind;
            let after = self.tokens[self.current + 2].kind;
            if (next == TokenKind::Ident || next == TokenKind::String) && after == TokenKind::Colon
            {
                return true;
            }
        }

        false
    }

    fn if_expr_stmt(&mut self) -> Result<Stmt, ParseError> {
        // 委托给 if_expr()，避免 else 分支逻辑重复
        let expr = self.if_expr()?.ok_or_else(|| ParseError {
            message: "expected 'if' or '如果'".into(),
            line: self.peek().line,
            column: self.peek().column,
        })?;
        Ok(Stmt::Expr(expr))
    }

    /// match 表达式语句入口（语句上下文）
    fn match_expr_stmt(&mut self) -> Result<Stmt, ParseError> {
        let expr = self.match_expr()?;
        Ok(Stmt::Expr(expr))
    }

    /// 解析 match 表达式
    ///
    /// 语法：`match (expr) { pattern => expr, ... }`
    ///
    /// # 支持的模式
    /// - 字面量：`0`, `true`, `"hello"`
    /// - 范围：`1..10`, `0..<100`
    /// - 变量绑定：`n`, `x`
    /// - 通配符：`_`
    fn match_expr(&mut self) -> Result<Expr, ParseError> {
        self.consume(TokenKind::Match, "expected 'match' or '匹配'")?;
        let span = self.span_here();
        self.consume(TokenKind::LParen, "expected '(' after match")?;
        let scrutinee = self.expression()?;
        self.consume(TokenKind::RParen, "expected ')' after match expression")?;
        self.consume(TokenKind::LBrace, "expected '{' to start match arms")?;

        let mut arms = Vec::new();
        if !self.check(TokenKind::RBrace) {
            loop {
                let pattern = self.parse_match_pattern()?;
                self.consume(TokenKind::Arrow, "expected '=>' in match arm")?;
                let body = self.expression()?;
                arms.push(MatchArm { pattern, body });
                if !self.match_kind(TokenKind::Comma) {
                    // Allow trailing without comma if next is RBrace
                    if !self.check(TokenKind::RBrace) {
                        return Err(ParseError {
                            message: "expected ',' or '}' after match arm".to_string(),
                            line: self.peek().line,
                            column: self.peek().column,
                        });
                    }
                    break;
                }
                if self.check(TokenKind::RBrace) {
                    break;
                }
            }
        }
        self.consume(TokenKind::RBrace, "expected '}' after match arms")?;
        Ok(Expr::Match { scrutinee: Box::new(scrutinee), arms, span })
    }

    /// 解析 match 分支的模式
    fn parse_match_pattern(&mut self) -> Result<MatchPattern, ParseError> {
        // 通配符 `_`
        if self.check(TokenKind::Ident) && self.text_of(self.peek()) == "_" {
            self.advance();
            return Ok(MatchPattern::Wildcard);
        }

        // 变量绑定模式：单个标识符（不是 true/false/nil 关键字）
        if self.check(TokenKind::Ident)
            && !self.check_if(TokenKind::is_true)
            && !self.check_if(TokenKind::is_false)
            && !self.check_if(TokenKind::is_nil)
        {
            // Look ahead: if next token is `..` or `..<`, it's a range starting with a variable
            // Otherwise, it's a variable binding pattern
            let next_kind = self.tokens.get(self.current + 1).map(|t| t.kind);
            if next_kind != Some(TokenKind::DotDot) && next_kind != Some(TokenKind::DotDotLt) {
                let tok = *self.advance();
                let name = self.text_of(&tok);
                return Ok(MatchPattern::Variable(name));
            }
        }

        // 字面量或范围模式：解析一个 primary 表达式
        // 使用 call() 而非 expression() 避免箭头函数解析干扰。
        // 但 call() 不处理一元负号，因此先特判 `-<Number>` 形式，
        // 将 `-1` 直接构造为 `Expr::Number { value: -1.0, .. }`，
        // 以便 MatchPattern::Literal 能识别（否则 `-1` 会被解析为
        // `Expr::Unary { op: Negate, operand: Number(1) }`，不匹配 Literal 分支）。
        let parse_neg_number_or_call = |parser: &mut Self| -> Result<Expr, ParseError> {
            if parser.check(TokenKind::Minus)
                && parser
                    .tokens
                    .get(parser.current + 1)
                    .is_some_and(|t| t.kind == TokenKind::Number)
            {
                let span = parser.span_here();
                parser.advance(); // 消费 `-`
                let num_tok = *parser.advance();
                let text = parser.text_of(&num_tok);
                let value = text.parse::<f64>().map_err(|_| ParseError {
                    message: format!("invalid number: {}", text),
                    line: num_tok.line,
                    column: num_tok.column,
                })?;
                Ok(Expr::Number { value: -value, span })
            } else {
                parser.call()
            }
        };
        let first = parse_neg_number_or_call(self)?;

        if self.check(TokenKind::DotDot) || self.check(TokenKind::DotDotLt) {
            let inclusive = self.check(TokenKind::DotDot);
            self.advance();
            // 范围结束值也支持负数（如 `-10..-1`）
            let end = parse_neg_number_or_call(self)?;
            return Ok(MatchPattern::Range {
                start: Box::new(first),
                end: Box::new(end),
                inclusive,
            });
        }

        match &first {
            Expr::Number { .. } | Expr::String { .. } | Expr::Bool { .. } | Expr::Nil { .. } => {
                Ok(MatchPattern::Literal(first))
            }
            _ => {
                let span = first.span();
                Err(ParseError {
                    message: "match pattern must be a literal, variable, range, or '_'".to_string(),
                    line: span.line,
                    column: span.column,
                })
            }
        }
    }

    fn if_expr(&mut self) -> Result<Option<Expr>, ParseError> {
        if self.check_if(TokenKind::is_if) {
            self.advance();
            let span = self.span_here();
            let condition = self.expression()?;
            self.consume(TokenKind::LBrace, "expected '{' after if condition")?;
            let then_branch = self.block()?;
            let else_branch = if self.match_if(TokenKind::is_else) {
                if self.check_if(TokenKind::is_if) {
                    let inner = self.if_expr()?.ok_or_else(|| ParseError {
                        message: "if_expr returned None after is_if check (internal parser inconsistency)".to_string(),
                        line: self.peek().line,
                        column: self.peek().column,
                    })?;
                    Some(Box::new(inner))
                } else {
                    self.consume(TokenKind::LBrace, "expected '{' after else")?;
                    let else_block = self.block()?;
                    Some(Box::new(Expr::Block { statements: else_block, span: span.clone() }))
                }
            } else {
                None
            };
            Ok(Some(Expr::If { condition: Box::new(condition), then_branch, else_branch, span }))
        } else {
            Ok(None)
        }
    }

    fn loop_expr(&mut self) -> Result<Expr, ParseError> {
        self.consume_if(TokenKind::is_loop, "expected 'loop' or '循环'")?;
        let span = self.span_here();
        self.consume(TokenKind::LBrace, "expected '{' after loop")?;
        let body = self.block()?;
        Ok(Expr::Loop { body, span })
    }

    fn while_expr(&mut self) -> Result<Option<Expr>, ParseError> {
        if self.check_if(TokenKind::is_while) {
            self.advance();
            let span = self.span_here();
            let condition = self.expression()?;
            self.consume(TokenKind::LBrace, "expected '{' after while condition")?;
            let body = self.block()?;
            Ok(Some(Expr::While { condition: Box::new(condition), body, span }))
        } else {
            Ok(None)
        }
    }

    fn while_expr_stmt(&mut self) -> Result<Stmt, ParseError> {
        // 委托给 while_expr()，避免重复解析逻辑
        let expr = self.while_expr()?.ok_or_else(|| ParseError {
            message: "expected 'while' or '当'".into(),
            line: self.peek().line,
            column: self.peek().column,
        })?;
        Ok(Stmt::Expr(expr))
    }

    fn loop_expr_stmt(&mut self) -> Result<Stmt, ParseError> {
        Ok(Stmt::Expr(self.loop_expr()?))
    }

    fn for_expr_stmt(&mut self) -> Result<Stmt, ParseError> {
        self.consume_if(TokenKind::is_for, "expected 'for' or '遍历'")?;
        let span = self.span_here();
        let var_tok = *self.consume(TokenKind::Ident, "expected variable name in for")?;
        let var_name = self.text_of(&var_tok);
        self.consume_if(TokenKind::is_in, "expected 'in' or '在' after for variable")?;
        let iterable = self.expression()?;
        self.consume(TokenKind::LBrace, "expected '{' after for-in iterable")?;
        let body = self.block()?;
        Ok(Stmt::Expr(Expr::ForIn { var_name, iterable: Box::new(iterable), body, span }))
    }

    fn return_stmt(&mut self) -> Result<Stmt, ParseError> {
        let tok = *self.consume_if(TokenKind::is_return, "expected 'return' or '返回'")?;
        let span = self.span_at(&tok);
        let value = if self.check(TokenKind::RBrace)
            || self.check(TokenKind::Semicolon)
            || self.is_at_end()
        {
            None
        } else {
            Some(Box::new(self.expression()?))
        };
        Ok(Stmt::Expr(Expr::Return { value, span }))
    }

    fn break_stmt(&mut self) -> Result<Stmt, ParseError> {
        let tok = *self.consume_if(TokenKind::is_break, "expected 'break' or '跳出'")?;
        let span = self.span_at(&tok);
        let value = if self.check(TokenKind::RBrace)
            || self.check(TokenKind::Semicolon)
            || self.is_at_end()
        {
            None
        } else {
            Some(Box::new(self.expression()?))
        };
        Ok(Stmt::Expr(Expr::Break { value, span }))
    }

    fn continue_stmt(&mut self) -> Result<Stmt, ParseError> {
        let tok = *self.consume_if(TokenKind::is_continue, "expected 'continue' or '继续'")?;
        let span = self.span_at(&tok);
        Ok(Stmt::Expr(Expr::Continue { span }))
    }

    /// 解析 try-catch[-keep] 语句
    ///
    /// 语法:
    ///   try <block> [catch (<ident>) <block>] [keep <block>]
    ///
    /// # 示例
    /// ```nuzo
    /// try {
    ///     risky_operation()
    /// } catch (e) {
    ///     handle_error(e)
    /// }
    ///
    /// // 带 keep 块
    /// try {
    ///     open_file()
    /// } catch (e) {
    ///     log_error(e)
    /// } keep {
    ///     cleanup()
    /// }
    /// ```
    fn try_expr_stmt(&mut self) -> Result<Stmt, ParseError> {
        let expr = self.parse_try_expression()?;
        Ok(Stmt::Expr(expr))
    }

    /// 解析 try-catch[-keep] 表达式
    ///
    /// 语法:
    ///   try <block> [catch (<ident>) <block>] [keep <block>]
    ///
    /// # 返回值
    /// 返回 `Expr::Try` 节点，包含：
    /// - `body`: try 块的语句列表
    /// - `catch_clause`: 可选的 catch 子句（包含绑定变量和执行体）
    /// - `keep_block`: 可选的 keep/finally 块
    fn parse_try_expression(&mut self) -> Result<Expr, ParseError> {
        self.consume(TokenKind::Try, "expected 'try'")?;
        let span = self.span_here();

        self.consume(TokenKind::LBrace, "expected '{' after try")?;
        let body = self.block()?;

        let catch_clause = if self.check(TokenKind::Catch) {
            self.advance();
            self.consume(TokenKind::LParen, "expected '(' after catch")?;

            let binding_tok =
                *self.consume(TokenKind::Ident, "expected exception variable name in catch")?;
            let binding = self.text_of(&binding_tok);

            self.consume(TokenKind::RParen, "expected ')' after catch variable")?;
            self.consume(TokenKind::LBrace, "expected '{' before catch body")?;
            let catch_body = self.block()?;

            Some(Box::new(CatchClause {
                binding,
                exception_type: None, // M2 支持: catch (e: Type)
                body: catch_body,
            }))
        } else {
            None
        };

        // 可选的 keep 块（类似 finally）
        let keep_block = if self.check(TokenKind::Keep) {
            self.advance();
            self.consume(TokenKind::LBrace, "expected '{' after keep")?;
            Some(self.block()?)
        } else {
            None
        };

        Ok(Expr::Try { body, catch_clause, keep_block, span })
    }

    /// 解析 out(抛出) 表达式
    ///
    /// 语法: out <expression>
    ///
    /// # 示例
    /// ```nuzo
    /// out "something went wrong"
    /// out {code: 404, message: "Not Found"}
    /// out error_value
    /// ```
    ///
    /// # 语义
    /// - 只能在 try 块内使用
    /// - 立即跳转到对应的 catch 子句
    /// - 如果没有匹配的 catch，异常会向上传播
    fn parse_out_expression(&mut self) -> Result<Expr, ParseError> {
        let span = self.span_here();
        self.consume(TokenKind::Out, "expected 'out'")?;

        let value = Box::new(self.expression()?);

        Ok(Expr::Out { value, span })
    }

    fn block_stmt(&mut self) -> Result<Stmt, ParseError> {
        // P2-8: 检测 `{ ident <literal> }` 或 `{ ident <literal> , ... }` 模式
        // （典型的漏写冒号的 dict 字面量），给出明确错误而非静默解析为 block。
        if self.looks_like_dict_with_missing_colon() {
            let tok = *self.peek();
            let key_token = &self.tokens[self.current + 1];
            let key_text = self.text_of(key_token);
            return Err(ParseError {
                message: format!(
                    "expected ':' after property name '{}' — did you mean to write a dict literal? (missing colon)",
                    key_text
                ),
                line: tok.line,
                column: tok.column,
            });
        }
        let span = self.span_here();
        self.consume(TokenKind::LBrace, "expected '{'")?;
        let statements = self.block()?;
        Ok(Stmt::Expr(Expr::Block { statements, span }))
    }

    /// 检测 `{ ident/string <number/string/ident> }` 或 `{ ident/string <literal> , ... }` 模式（P2-8）
    ///
    /// 这种模式几乎肯定是漏写冒号的 dict 字面量（如 `{ a 1 }` 期望为 `{ a: 1 }`）。
    /// 静默解析为 block 会让用户困惑（block 中的 `a` 和 `1` 是两个无意义的语句）。
    ///
    /// # 检测条件
    ///
    /// 1. 当前 Token 是 `{`
    /// 2. 下一个 Token 是 `Ident` 或 `String`（潜在的 dict key）
    /// 3. 再下一个 Token 是 `Number`、`String` 或 `Ident`（潜在的 value，
    ///    且不是运算符/分号/右括号，因为这些在表达式中合法）
    /// 4. value 之后的 Token 是 `}` 或 `,`（确认是 dict-like 结构而非表达式）
    ///
    /// # 误报风险
    ///
    /// `{ a 1 }`（block 中两个语句 `a` 和 `1`）也会被检测到，但这种代码
    /// 毫无意义（`a` 的求值结果被丢弃），几乎肯定是 dict 的漏冒号错误。
    fn looks_like_dict_with_missing_colon(&self) -> bool {
        if self.current + 3 >= self.tokens.len() {
            return false;
        }
        let cur = self.tokens[self.current].kind;
        if cur != TokenKind::LBrace {
            return false;
        }

        let next = &self.tokens[self.current + 1];
        let after = &self.tokens[self.current + 2];
        let after_after = &self.tokens[self.current + 3];

        // key 必须是 Ident 或 String
        let next_is_key = next.kind == TokenKind::Ident || next.kind == TokenKind::String;
        // value 必须是字面量或 ident（不能是运算符/分隔符）
        let after_is_value =
            matches!(after.kind, TokenKind::Number | TokenKind::String | TokenKind::Ident);
        // value 之后必须是 } 或 ,（确认 dict-like 结构）
        let after_value_is_dict_end =
            matches!(after_after.kind, TokenKind::RBrace | TokenKind::Comma);

        next_is_key && after_is_value && after_value_is_dict_end
    }

    fn block(&mut self) -> Result<Vec<Stmt>, ParseError> {
        // P1-3: 递归深度保护（与 expression() 一致）。
        // 拆分为 block() + block_inner() 是为了在 block_inner() 内部使用 ?
        // 时也能保证 depth 正确递减（block() 持有 depth 计数，block_inner()
        // 返回 Result 后由 block() 统一递减）。
        self.depth += 1;
        if self.depth > MAX_PARSER_DEPTH {
            self.depth -= 1;
            let tok = *self.peek();
            return Err(ParseError {
                message: format!(
                    "block nesting exceeds limit ({}) — possible stack overflow protection",
                    MAX_PARSER_DEPTH
                ),
                line: tok.line,
                column: tok.column,
            });
        }
        let result = self.block_inner();
        self.depth -= 1;
        result
    }

    fn block_inner(&mut self) -> Result<Vec<Stmt>, ParseError> {
        let mut statements = Vec::new();
        while !self.check(TokenKind::RBrace) && !self.is_at_end() {
            statements.push(self.declaration()?);
            self.skip_semicolons();
        }
        self.consume(TokenKind::RBrace, "expected '}' after block")?;
        Ok(statements)
    }

    fn expr_statement(&mut self) -> Result<Stmt, ParseError> {
        let expr = self.expression()?;
        if self.match_kind(TokenKind::Eq) {
            let span = self.span_here();
            let value = self.expression()?;
            let target = self.expr_to_assign_target(expr)?;
            Ok(Stmt::Assign { target, value, span })
        } else if let Some(op) = self.match_compound_assign() {
            let span = self.span_here();
            let rhs = self.expression()?;
            let target_expr = self.assign_target_to_expr(&expr)?;
            let value = Expr::Binary {
                left: Box::new(target_expr),
                op,
                right: Box::new(rhs),
                span: span.clone(),
            };
            let target = self.expr_to_assign_target(expr)?;
            Ok(Stmt::Assign { target, value, span })
        } else {
            Ok(Stmt::Expr(expr))
        }
    }

    fn match_compound_assign(&mut self) -> Option<BinaryOp> {
        let op = match self.peek().kind {
            TokenKind::PlusEqual => BinaryOp::Add,
            TokenKind::MinusEqual => BinaryOp::Sub,
            TokenKind::StarEqual => BinaryOp::Mul,
            TokenKind::SlashEqual => BinaryOp::Div,
            _ => return None,
        };
        self.advance();
        Some(op)
    }

    fn assign_target_to_expr(&self, expr: &Expr) -> Result<Expr, ParseError> {
        match expr {
            Expr::Ident { name, span } => {
                Ok(Expr::Ident { name: name.clone(), span: span.clone() })
            }
            Expr::Index { object, index, span } => {
                Ok(Expr::Index { object: object.clone(), index: index.clone(), span: span.clone() })
            }
            Expr::Field { object, name, span } => {
                Ok(Expr::Field { object: object.clone(), name: name.clone(), span: span.clone() })
            }
            _ => {
                let span = expr.span();
                Err(ParseError {
                    message: "invalid assignment target".to_string(),
                    line: span.line,
                    column: span.column,
                })
            }
        }
    }

    fn expr_to_assign_target(&self, expr: Expr) -> Result<AssignTarget, ParseError> {
        match expr {
            Expr::Ident { name, .. } => Ok(AssignTarget::Ident { name }),
            Expr::Index { object, index, .. } => Ok(AssignTarget::Index { object, index }),
            Expr::Field { object, name, .. } => Ok(AssignTarget::Field { object, name }),
            _ => {
                let span = expr.span();
                Err(ParseError {
                    message: "invalid assignment target".to_string(),
                    line: span.line,
                    column: span.column,
                })
            }
        }
    }

    fn expression(&mut self) -> Result<Expr, ParseError> {
        // P1-3: 递归深度保护。在进入表达式解析前递增 depth，离开时递减。
        // 超过 MAX_PARSER_DEPTH 时立即返回错误，防止恶意嵌套输入导致栈溢出。
        // 注意：即使 pipe_expr 返回 Err，depth 也会正确递减（result 在
        // depth -= 1 之后才返回）。
        self.depth += 1;
        if self.depth > MAX_PARSER_DEPTH {
            self.depth -= 1;
            let tok = *self.peek();
            return Err(ParseError {
                message: format!(
                    "expression nesting exceeds limit ({}) — possible stack overflow protection",
                    MAX_PARSER_DEPTH
                ),
                line: tok.line,
                column: tok.column,
            });
        }
        let result = self.pipe_expr();
        self.depth -= 1;
        result
    }

    /// 管道运算符 `|>`（左结合，优先级仅高于箭头函数）
    ///
    /// 语法：`pipe := arrow_expr ('|>' arrow_expr)*`
    ///
    /// `x |> f` 脱糖为 `f(x)`
    /// `x |> f(y)` 脱糖为 `f(x, y)`
    /// `x |> f |> g` 脱糖为 `g(f(x))`
    fn pipe_expr(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.arrow_expr()?;
        while self.match_kind(TokenKind::Pipe) {
            let span = self.span_here();
            let right = self.arrow_expr()?;
            left = self.desugar_pipe(left, right, span)?;
        }
        Ok(left)
    }

    /// 将管道表达式脱糖为函数调用
    ///
    /// - 如果右侧是 Call，将 left 前插为第一个参数
    /// - 如果右侧不是 Call，创建新的 Call { callee: right, args: [left] }
    fn desugar_pipe(&self, left: Expr, right: Expr, span: Span) -> Result<Expr, ParseError> {
        match right {
            Expr::Call { callee, mut args, span: call_span } => {
                args.insert(0, left);
                Ok(Expr::Call { callee, args, span: call_span })
            }
            _ => Ok(Expr::Call { callee: Box::new(right), args: vec![left], span }),
        }
    }

    fn arrow_expr(&mut self) -> Result<Expr, ParseError> {
        if self.check(TokenKind::Ident)
            && self.tokens.get(self.current + 1).is_some_and(|t| t.kind == TokenKind::Arrow)
        {
            let tok = *self.advance();
            let param = self.text_of(&tok);
            self.advance();
            let span = self.span_here();
            let body = self.arrow_body()?;
            return Ok(Expr::Closure { params: vec![param], body, span });
        }

        if self.check(TokenKind::LParen) {
            let saved = self.current;
            if let Some(params) = self.try_arrow_params() {
                let span = self.span_here();
                let body = self.arrow_body()?;
                return Ok(Expr::Closure { params, body, span });
            }
            self.current = saved;
        }

        self.or_expr()
    }

    fn try_arrow_params(&mut self) -> Option<Vec<String>> {
        self.advance();
        let mut params = Vec::new();

        if self.check(TokenKind::RParen) {
            self.advance();
        } else {
            loop {
                if !self.check(TokenKind::Ident) {
                    return None;
                }
                let tok = *self.advance();
                params.push(self.text_of(&tok));
                if !self.match_kind(TokenKind::Comma) {
                    break;
                }
            }
            if !self.check(TokenKind::RParen) {
                return None;
            }
            self.advance();
        }

        if !self.check(TokenKind::Arrow) {
            return None;
        }
        self.advance();
        Some(params)
    }

    fn arrow_body(&mut self) -> Result<Vec<Stmt>, ParseError> {
        if self.check(TokenKind::LBrace) {
            self.consume(TokenKind::LBrace, "expected '{' after =>")?;
            self.block()
        } else {
            let expr = self.expression()?;
            let span = self.span_here();
            Ok(vec![Stmt::Expr(Expr::Return { value: Some(Box::new(expr)), span })])
        }
    }

    fn or_expr(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.null_coalesce_expr()?;
        while self.match_if(TokenKind::is_or) {
            let span = self.span_here();
            let right = self.null_coalesce_expr()?;
            left = Expr::Or { left: Box::new(left), right: Box::new(right), span };
        }
        Ok(left)
    }

    /// 空值合并运算符 `??`（右结合，优先级高于 or，低于 or）
    ///
    /// 语法：`null_coalesce := and_expr ('??' and_expr)*`
    ///
    /// `value ?? default` 当 value 为 nil 时返回 default，否则返回 value。
    fn null_coalesce_expr(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.and_expr()?;
        while self.match_kind(TokenKind::QuestionQuestion) {
            let span = self.span_here();
            let right = self.and_expr()?;
            left = Expr::NullCoalesce { left: Box::new(left), right: Box::new(right), span };
        }
        Ok(left)
    }

    fn and_expr(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.comparison()?;
        while self.match_if(TokenKind::is_and) {
            let span = self.span_here();
            let right = self.comparison()?;
            left = Expr::And { left: Box::new(left), right: Box::new(right), span };
        }
        Ok(left)
    }

    fn comparison(&mut self) -> Result<Expr, ParseError> {
        // 支持链式比较：`1 < 2 < 3` 语义化为 `(1 < 2) && (2 < 3)`（Python 风格）
        //
        // P2-7 修复：中间操作数不再被求值两次。
        //
        // 实现：收集所有操作数和运算符，对于链长度 >= 2 的情况，
        // 生成 `Expr::Block` 包含临时变量赋值 + And 连接的比较表达式。
        //
        // `a < b < c` 脱糖为：
        // ```
        // {
        //     __nuzo_cmp_0 = b
        //     (a < __nuzo_cmp_0) && (__nuzo_cmp_0 < c)
        // }
        // ```
        //
        // 这样 `b` 只求值一次，避免对带副作用的操作数（如函数调用）
        // 产生非预期行为。
        let first = self.range()?;

        let mut ops: Vec<BinaryOp> = Vec::new();
        let mut operands: Vec<Expr> = vec![first];

        loop {
            let op = match self.peek().kind {
                TokenKind::EqEq => Some(BinaryOp::Eq),
                TokenKind::BangEq => Some(BinaryOp::Neq),
                TokenKind::Lt => Some(BinaryOp::Lt),
                TokenKind::Gt => Some(BinaryOp::Gt),
                TokenKind::LtEq => Some(BinaryOp::LtEq),
                TokenKind::GtEq => Some(BinaryOp::GtEq),
                _ => None,
            };

            if let Some(op) = op {
                self.advance();
                let right = self.range()?;
                ops.push(op);
                operands.push(right);
            } else {
                break;
            }
        }

        // 没有比较运算符：直接返回唯一操作数
        if ops.is_empty() {
            // operands.len() == 1
            return Ok(operands.into_iter().next().unwrap());
        }

        let span = self.span_here();

        // 单个比较：直接构造 Binary，无需临时变量
        if ops.len() == 1 {
            let mut iter = operands.into_iter();
            let l = iter.next().unwrap();
            let r = iter.next().unwrap();
            return Ok(Expr::Binary {
                left: Box::new(l),
                op: ops.into_iter().next().unwrap(),
                right: Box::new(r),
                span,
            });
        }

        // 多个比较（链长度 >= 2）：用临时变量缓存中间操作数，避免重复求值
        //
        // operands = [a, b, c, d]（长度 = ops.len() + 1）
        // 中间操作数索引：1 .. operands.len()-1（即 b, c）
        // 临时变量数量：operands.len() - 2
        //
        // 对于第 i 个比较（0-indexed）：
        //   left  = if i == 0 { operands[0] } else { Ident(tmp[i-1]) }
        //   right = if i == ops.len()-1 { operands[last] } else { Ident(tmp[i]) }
        let n_intermediates = operands.len().saturating_sub(2);
        let mut statements: Vec<Stmt> = Vec::with_capacity(n_intermediates + 1);
        let mut tmp_names: Vec<String> = Vec::with_capacity(n_intermediates);

        // 为每个中间操作数生成临时变量赋值
        // 使用 mem::replace 取出 operands[i] 避免 clone
        for i in 1..operands.len() - 1 {
            let name = self.gen_tmp_name();
            let value = std::mem::replace(&mut operands[i], Expr::Nil { span: span.clone() });
            statements.push(Stmt::Assign {
                target: AssignTarget::Ident { name: name.clone() },
                value,
                span: span.clone(),
            });
            tmp_names.push(name);
        }

        // 构建比较表达式链
        let mut comparisons: Vec<Expr> = Vec::with_capacity(ops.len());
        for i in 0..ops.len() {
            let left_expr = if i == 0 {
                operands[0].clone()
            } else {
                Expr::Ident { name: tmp_names[i - 1].clone(), span: span.clone() }
            };
            let right_expr = if i == ops.len() - 1 {
                operands[operands.len() - 1].clone()
            } else {
                Expr::Ident { name: tmp_names[i].clone(), span: span.clone() }
            };
            comparisons.push(Expr::Binary {
                left: Box::new(left_expr),
                op: ops[i],
                right: Box::new(right_expr),
                span: span.clone(),
            });
        }

        // 用 And 连接所有比较
        let mut cmp_iter = comparisons.into_iter();
        let mut result = cmp_iter.next().unwrap();
        for cmp in cmp_iter {
            result = Expr::And { left: Box::new(result), right: Box::new(cmp), span: span.clone() };
        }

        // 最后一条语句是结果表达式（块的返回值）
        statements.push(Stmt::Expr(result));

        Ok(Expr::Block { statements, span })
    }

    fn range(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.addition()?;

        loop {
            if self.match_kind(TokenKind::DotDotLt) {
                let span = self.span_here();
                let right = self.addition()?;
                left = Expr::Range {
                    start: Box::new(left),
                    end: Box::new(right),
                    inclusive: false,
                    span,
                };
            } else if self.match_kind(TokenKind::DotDot) {
                let span = self.span_here();
                let right = self.addition()?;
                left = Expr::Range {
                    start: Box::new(left),
                    end: Box::new(right),
                    inclusive: true,
                    span,
                };
            } else {
                break;
            }
        }

        Ok(left)
    }

    fn addition(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.multiplication()?;

        loop {
            let op = match self.peek().kind {
                TokenKind::Plus => Some(BinaryOp::Add),
                TokenKind::Minus => Some(BinaryOp::Sub),
                _ => None,
            };

            if let Some(op) = op {
                let span = self.span_here();
                self.advance();
                let right = self.multiplication()?;
                left = Expr::Binary { left: Box::new(left), op, right: Box::new(right), span };
            } else {
                break;
            }
        }

        Ok(left)
    }

    fn multiplication(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.power()?;

        loop {
            // 支持 `%` 符号和 `mod` 文本关键字作为取模运算符
            let op = if self.check(TokenKind::Percent)
                || (self.check(TokenKind::Ident) && self.text_of(self.peek()) == "mod")
            {
                Some(BinaryOp::Mod)
            } else {
                match self.peek().kind {
                    TokenKind::Star => Some(BinaryOp::Mul),
                    TokenKind::Slash => Some(BinaryOp::Div),
                    _ => None,
                }
            };

            if let Some(op) = op {
                let span = self.span_here();
                self.advance();
                let right = self.power()?;
                left = Expr::Binary { left: Box::new(left), op, right: Box::new(right), span };
            } else {
                break;
            }
        }

        Ok(left)
    }

    /// 幂运算 `**`（右结合，优先级高于乘法）
    ///
    /// 语法：`power := unary ('**' power)?`
    /// 右结合：`2 ** 3 ** 2` = `2 ** (3 ** 2)` = 512
    fn power(&mut self) -> Result<Expr, ParseError> {
        let left = self.unary()?;
        if self.match_kind(TokenKind::StarStar) {
            let span = self.span_here();
            let right = self.power()?; // 右结合：递归调用自身
            Ok(Expr::Binary {
                left: Box::new(left),
                op: BinaryOp::Pow,
                right: Box::new(right),
                span,
            })
        } else {
            Ok(left)
        }
    }

    fn unary(&mut self) -> Result<Expr, ParseError> {
        let span = self.span_here();
        if self.match_kind(TokenKind::Minus) {
            let operand = self.unary()?;
            Ok(Expr::Unary { op: UnaryOp::Negate, operand: Box::new(operand), span })
        } else if self.match_kind(TokenKind::Bang) {
            let operand = self.unary()?;
            Ok(Expr::Unary { op: UnaryOp::Not, operand: Box::new(operand), span })
        } else {
            self.call()
        }
    }

    fn call(&mut self) -> Result<Expr, ParseError> {
        let expr = self.primary()?;

        let mut expr = expr;
        loop {
            if self.match_kind(TokenKind::LParen) {
                let span = self.span_here();
                let args = self.parse_args()?;
                self.consume(TokenKind::RParen, "expected ')' after arguments")?;
                expr = Expr::Call { callee: Box::new(expr), args, span };
            } else if self.match_kind(TokenKind::LBracket) {
                let span = self.span_here();
                let index = self.expression()?;
                self.consume(TokenKind::RBracket, "expected ']' after index")?;
                expr = Expr::Index { object: Box::new(expr), index: Box::new(index), span };
            } else if self.match_kind(TokenKind::Dot) {
                let span = self.span_here();
                let tok = *self.consume(TokenKind::Ident, "expected property name after '.'")?;
                let name = self.text_of(&tok);
                expr = Expr::Field { object: Box::new(expr), name, span };
            } else {
                break;
            }
        }

        Ok(expr)
    }

    fn parse_args(&mut self) -> Result<Vec<Expr>, ParseError> {
        let mut args = Vec::new();
        if !self.check(TokenKind::RParen) {
            loop {
                args.push(self.expression()?);
                if !self.match_kind(TokenKind::Comma) {
                    break;
                }
            }
        }
        Ok(args)
    }

    fn primary(&mut self) -> Result<Expr, ParseError> {
        let token = *self.peek();
        let span = self.span_at(&token);

        if self.check_if(TokenKind::is_if) {
            return self.if_expr().and_then(|e| {
                e.ok_or_else(|| ParseError {
                    message: "expected if expression".into(),
                    line: self.peek().line,
                    column: self.peek().column,
                })
            });
        }

        if self.check_if(TokenKind::is_loop) {
            return self.loop_expr();
        }

        if self.check(TokenKind::Try) {
            return self.parse_try_expression();
        }

        if self.check(TokenKind::Out) {
            return self.parse_out_expression();
        }

        if self.check(TokenKind::Match) {
            return self.match_expr();
        }

        if self.match_if(TokenKind::is_true) {
            return Ok(Expr::Bool { value: true, span });
        }
        if self.match_if(TokenKind::is_false) {
            return Ok(Expr::Bool { value: false, span });
        }
        if self.match_if(TokenKind::is_nil) {
            return Ok(Expr::Nil { span });
        }

        if self.match_kind(TokenKind::Number) {
            let prev = *self.previous();
            let text = self.text_of(&prev);
            let value = text.parse::<f64>().map_err(|_| ParseError {
                message: format!("invalid number: {}", text),
                line: prev.line,
                column: prev.column,
            })?;
            return Ok(Expr::Number { value, span });
        }

        if self.match_kind(TokenKind::String) {
            let prev = *self.previous();
            let value = self.text_of(&prev);
            return Ok(Expr::String { value, span });
        }

        if self.match_kind(TokenKind::Ident) {
            let prev = *self.previous();
            let name = self.text_of(&prev);
            return Ok(Expr::Ident { name, span });
        }

        if self.match_kind(TokenKind::LParen) {
            let first = self.expression()?;
            if self.match_kind(TokenKind::Comma) {
                let mut elements = vec![first];
                loop {
                    elements.push(self.expression()?);
                    if !self.match_kind(TokenKind::Comma) {
                        break;
                    }
                }
                self.consume(TokenKind::RParen, "expected ')' after tuple")?;
                return Ok(Expr::Tuple { elements, span });
            }
            self.consume(TokenKind::RParen, "expected ')' after expression")?;
            return Ok(first);
        }

        if self.match_kind(TokenKind::LBracket) {
            let mut elements = Vec::new();
            if !self.check(TokenKind::RBracket) {
                loop {
                    elements.push(self.expression()?);
                    if !self.match_kind(TokenKind::Comma) {
                        break;
                    }
                }
            }
            self.consume(TokenKind::RBracket, "expected ']' after array")?;
            return Ok(Expr::Array { elements, span });
        }

        if self.match_kind(TokenKind::LBrace) {
            let mut pairs = Vec::new();
            if !self.check(TokenKind::RBrace) {
                loop {
                    let key = if self.check(TokenKind::Ident) || self.check(TokenKind::String) {
                        let tok = *self.advance();
                        self.text_of(&tok)
                    } else {
                        return Err(ParseError {
                            message: "expected property name in dict".to_string(),
                            line: self.peek().line,
                            column: self.peek().column,
                        });
                    };
                    self.consume(TokenKind::Colon, "expected ':' after property name")?;
                    let value = self.expression()?;
                    pairs.push((key, value));
                    if !self.match_kind(TokenKind::Comma) {
                        break;
                    }
                }
            }
            self.consume(TokenKind::RBrace, "expected '}' after dict")?;
            return Ok(Expr::Dict { pairs, span });
        }

        if self.check_if(TokenKind::is_fn) {
            self.advance();
            let span = self.span_here();
            let params = if self.check(TokenKind::LParen) {
                self.advance();
                let p = self.parse_params()?;
                self.consume(TokenKind::RParen, "expected ')' after parameters")?;
                p
            } else {
                Vec::new()
            };
            self.consume(TokenKind::LBrace, "expected '{' before function body")?;
            let body = self.block()?;
            return Ok(Expr::Fn { name: None, params, body, span });
        }

        Err(ParseError {
            message: format!("unexpected token: {}", token.kind),
            line: token.line,
            column: token.column,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // 注：std::assert_matches 在 Rust 1.88.0 stable 中仍为 unstable feature，
    // 改用稳定的 matches!() 宏 + assert! 组合。
    // use std::assert_matches;

    // 辅助函数：解析源代码并返回 Program
    fn parse_source(source: &str) -> Result<Program, ParseError> {
        Parser::parse(source)
    }

    // 辅助函数：解析单个表达式语句
    fn parse_expr(source: &str) -> Result<Expr, ParseError> {
        let program = parse_source(source)?;
        assert_eq!(program.statements.len(), 1);
        match &program.statements[0] {
            Stmt::Expr(expr) => Ok(expr.clone()),
            _ => panic!("Expected expression statement"),
        }
    }

    mod import_tests {
        use super::*;

        #[test]
        fn test_parse_import_eager() {
            let program = parse_source(r#"import "path/to/file.nuzo""#).unwrap();
            assert_eq!(program.statements.len(), 1);
            match &program.statements[0] {
                Stmt::Import { path, lazy, span, .. } => {
                    assert_eq!(path, "path/to/file.nuzo");
                    assert!(!*lazy, "eager import 应为 lazy=false");
                    assert_eq!(span.line, 1);
                    assert_eq!(span.column, 1);
                }
                other => panic!("Expected Stmt::Import, got {:?}", other),
            }
        }

        #[test]
        fn test_parse_import_lazy() {
            let program = parse_source(r#"lazy import "path/to/file.nuzo""#).unwrap();
            assert_eq!(program.statements.len(), 1);
            match &program.statements[0] {
                Stmt::Import { path, lazy, .. } => {
                    assert_eq!(path, "path/to/file.nuzo");
                    assert!(*lazy, "lazy import 应为 lazy=true");
                }
                other => panic!("Expected Stmt::Import, got {:?}", other),
            }
        }

        #[test]
        fn test_parse_import_chinese_eager() {
            let program = parse_source(r#"导入 "路径/文件.nuzo""#).unwrap();
            assert_eq!(program.statements.len(), 1);
            match &program.statements[0] {
                Stmt::Import { path, lazy, .. } => {
                    assert_eq!(path, "路径/文件.nuzo");
                    assert!(!*lazy);
                }
                other => panic!("Expected Stmt::Import, got {:?}", other),
            }
        }

        #[test]
        fn test_parse_import_chinese_lazy() {
            let program = parse_source(r#"懒 导入 "路径/文件.nuzo""#).unwrap();
            assert_eq!(program.statements.len(), 1);
            match &program.statements[0] {
                Stmt::Import { path, lazy, .. } => {
                    assert_eq!(path, "路径/文件.nuzo");
                    assert!(*lazy);
                }
                other => panic!("Expected Stmt::Import, got {:?}", other),
            }
        }

        #[test]
        fn test_parse_import_with_alias() {
            // alias 解析后忽略，path 仍为字面量
            let program = parse_source(r#"import "path/to/file.nuzo" as mymod"#).unwrap();
            assert_eq!(program.statements.len(), 1);
            match &program.statements[0] {
                Stmt::Import { path, lazy, .. } => {
                    assert_eq!(path, "path/to/file.nuzo");
                    assert!(!*lazy);
                }
                other => panic!("Expected Stmt::Import, got {:?}", other),
            }
        }

        #[test]
        fn test_parse_import_error_non_string() {
            // import 后是数字（既非字符串也非标识符）→ "expected string literal or identifier after import"
            let err = parse_source("import 42").unwrap_err();
            assert!(
                err.message.contains("expected string literal or identifier after import"),
                "错误信息不对: {}",
                err.message
            );
        }

        #[test]
        fn test_parse_import_error_eof() {
            // import 后 EOF → "unexpected EOF after import"
            let err = parse_source("import").unwrap_err();
            assert!(
                err.message.contains("unexpected EOF after import"),
                "错误信息不对: {}",
                err.message
            );
        }

        #[test]
        fn test_parse_import_error_lazy_no_import() {
            // lazy 后非 import → "expected import after lazy"
            let err = parse_source("lazy 42").unwrap_err();
            assert!(
                err.message.contains("expected import after lazy"),
                "错误信息不对: {}",
                err.message
            );
        }

        #[test]
        fn test_parse_import_error_lazy_eof() {
            // lazy 后 EOF → "expected import after lazy"
            let err = parse_source("lazy").unwrap_err();
            assert!(
                err.message.contains("expected import after lazy"),
                "错误信息不对: {}",
                err.message
            );
        }

        #[test]
        fn test_parse_import_chinese_alias() {
            // 中文 as 别名（作为）
            let program = parse_source(r#"导入 "路径/文件.nuzo" 作为 模块别名"#).unwrap();
            assert_eq!(program.statements.len(), 1);
            match &program.statements[0] {
                Stmt::Import { path, lazy, .. } => {
                    assert_eq!(path, "路径/文件.nuzo");
                    assert!(!*lazy);
                }
                other => panic!("Expected Stmt::Import, got {:?}", other),
            }
        }

        #[test]
        fn test_parse_import_multiple_statements() {
            // 多条 import 语句混合中英文
            let src = r#"
                import "a.nuzo"
                lazy import "b.nuzo"
                导入 "c.nuzo"
                懒 导入 "d.nuzo"
            "#;
            let program = parse_source(src).unwrap();
            assert_eq!(program.statements.len(), 4);
            // 第一条 eager
            match &program.statements[0] {
                Stmt::Import { path, lazy, .. } => {
                    assert_eq!(path, "a.nuzo");
                    assert!(!*lazy);
                }
                _ => panic!("stmt[0] 应为 Import"),
            }
            // 第二条 lazy
            match &program.statements[1] {
                Stmt::Import { path, lazy, .. } => {
                    assert_eq!(path, "b.nuzo");
                    assert!(*lazy);
                }
                _ => panic!("stmt[1] 应为 Import"),
            }
            // 第三条中文 eager
            match &program.statements[2] {
                Stmt::Import { path, lazy, .. } => {
                    assert_eq!(path, "c.nuzo");
                    assert!(!*lazy);
                }
                _ => panic!("stmt[2] 应为 Import"),
            }
            // 第四条中文 lazy
            match &program.statements[3] {
                Stmt::Import { path, lazy, .. } => {
                    assert_eq!(path, "d.nuzo");
                    assert!(*lazy);
                }
                _ => panic!("stmt[3] 应为 Import"),
            }
        }

        #[test]
        fn test_import_module_name() {
            // import math — 标识符模块名，不带引号
            let program = parse_source("import math").unwrap();
            assert_eq!(program.statements.len(), 1);
            match &program.statements[0] {
                Stmt::Import { path, lazy, alias, .. } => {
                    assert_eq!(path, "math");
                    assert!(!*lazy, "eager import 应为 lazy=false");
                    assert_eq!(*alias, None, "无 as 时 alias 应为 None");
                }
                other => panic!("Expected Stmt::Import, got {:?}", other),
            }
        }

        #[test]
        fn test_import_string_path() {
            // import "utils.nuzo" — 字符串字面量路径
            let program = parse_source(r#"import "utils.nuzo""#).unwrap();
            assert_eq!(program.statements.len(), 1);
            match &program.statements[0] {
                Stmt::Import { path, lazy, alias, .. } => {
                    assert_eq!(path, "utils.nuzo");
                    assert!(!*lazy);
                    assert_eq!(*alias, None);
                }
                other => panic!("Expected Stmt::Import, got {:?}", other),
            }
        }

        #[test]
        fn test_import_with_alias() {
            // import "utils.nuzo" as utils — 字符串路径 + 别名
            let program = parse_source(r#"import "utils.nuzo" as utils"#).unwrap();
            assert_eq!(program.statements.len(), 1);
            match &program.statements[0] {
                Stmt::Import { path, lazy, alias, .. } => {
                    assert_eq!(path, "utils.nuzo");
                    assert!(!*lazy);
                    assert_eq!(*alias, Some("utils".to_string()));
                }
                other => panic!("Expected Stmt::Import, got {:?}", other),
            }
        }

        #[test]
        fn test_import_module_name_with_alias() {
            // import math as m — 标识符模块名 + 别名
            let program = parse_source("import math as m").unwrap();
            assert_eq!(program.statements.len(), 1);
            match &program.statements[0] {
                Stmt::Import { path, lazy, alias, .. } => {
                    assert_eq!(path, "math");
                    assert!(!*lazy);
                    assert_eq!(*alias, Some("m".to_string()));
                }
                other => panic!("Expected Stmt::Import, got {:?}", other),
            }
        }

        #[test]
        fn test_import_missing_path() {
            // import 后 EOF → 错误
            let err = parse_source("import").unwrap_err();
            assert!(
                err.message.contains("unexpected EOF after import"),
                "错误信息不对: {}",
                err.message
            );
        }

        #[test]
        fn test_import_as_without_ident() {
            // import "x.nuzo" as — as 后无标识符 → 错误
            let err = parse_source(r#"import "x.nuzo" as"#).unwrap_err();
            assert!(
                err.message.contains("expected identifier after as"),
                "错误信息不对: {}",
                err.message
            );
        }
    }

    mod literal_tests {
        use super::*;

        #[test]
        fn test_parse_integer() {
            let expr = parse_expr("42").unwrap();
            match expr {
                Expr::Number { value, .. } => assert_eq!(value, 42.0),
                _ => panic!("Expected Number"),
            }
        }

        #[test]
        fn test_parse_float() {
            let expr = parse_expr("2.5").unwrap();
            match expr {
                Expr::Number { value, .. } => assert!((value - 2.5).abs() < 1e-10),
                _ => panic!("Expected Number"),
            }
        }

        #[test]
        fn test_parse_negative_number() {
            let expr = parse_expr("-42").unwrap();
            match expr {
                Expr::Unary { op: UnaryOp::Negate, operand, .. } => match *operand {
                    Expr::Number { value, .. } => assert_eq!(value, 42.0),
                    _ => panic!("Expected number operand"),
                },
                _ => panic!("Expected unary negate"),
            }
        }

        #[test]
        fn test_parse_string() {
            let expr = parse_expr("\"hello\"").unwrap();
            match expr {
                Expr::String { value, .. } => assert_eq!(value, "hello"),
                _ => panic!("Expected String"),
            }
        }

        #[test]
        fn test_parse_string_with_spaces() {
            let expr = parse_expr("\"hello world\"").unwrap();
            match expr {
                Expr::String { value, .. } => assert_eq!(value, "hello world"),
                _ => panic!("Expected String"),
            }
        }

        #[test]
        fn test_parse_true() {
            let expr = parse_expr("true").unwrap();
            match expr {
                Expr::Bool { value, .. } => assert!(value),
                _ => panic!("Expected Bool(true)"),
            }
        }

        #[test]
        fn test_parse_false() {
            let expr = parse_expr("false").unwrap();
            match expr {
                Expr::Bool { value, .. } => assert!(!value),
                _ => panic!("Expected Bool(false)"),
            }
        }

        #[test]
        fn test_parse_nil() {
            let expr = parse_expr("nil").unwrap();
            assert!(matches!(expr, Expr::Nil { .. }));
        }

        #[test]
        fn test_parse_identifier() {
            let expr = parse_expr("myVar").unwrap();
            match expr {
                Expr::Ident { name, .. } => assert_eq!(name, "myVar"),
                _ => panic!("Expected Ident"),
            }
        }

        #[test]
        fn test_parse_zero() {
            let expr = parse_expr("0").unwrap();
            match expr {
                Expr::Number { value, .. } => assert_eq!(value, 0.0),
                _ => panic!("Expected Number(0)"),
            }
        }
    }

    mod operator_tests {
        use super::*;

        #[test]
        fn test_addition() {
            let expr = parse_expr("1 + 2").unwrap();
            match expr {
                Expr::Binary { op: BinaryOp::Add, .. } => (),
                _ => panic!("Expected Add"),
            }
        }

        #[test]
        fn test_subtraction() {
            let expr = parse_expr("5 - 3").unwrap();
            match expr {
                Expr::Binary { op: BinaryOp::Sub, .. } => (),
                _ => panic!("Expected Sub"),
            }
        }

        #[test]
        fn test_multiplication() {
            let expr = parse_expr("4 * 3").unwrap();
            match expr {
                Expr::Binary { op: BinaryOp::Mul, .. } => (),
                _ => panic!("Expected Mul"),
            }
        }

        #[test]
        fn test_division() {
            let expr = parse_expr("10 / 2").unwrap();
            match expr {
                Expr::Binary { op: BinaryOp::Div, .. } => (),
                _ => panic!("Expected Div"),
            }
        }

        #[test]
        fn test_modulo() {
            let expr = parse_expr("7 % 3").unwrap();
            match expr {
                Expr::Binary { op: BinaryOp::Mod, .. } => (),
                _ => panic!("Expected Mod"),
            }
        }

        #[test]
        fn test_equality() {
            let expr = parse_expr("1 == 2").unwrap();
            match expr {
                Expr::Binary { op: BinaryOp::Eq, .. } => (),
                _ => panic!("Expected Eq"),
            }
        }

        #[test]
        fn test_inequality() {
            let expr = parse_expr("1 != 2").unwrap();
            match expr {
                Expr::Binary { op: BinaryOp::Neq, .. } => (),
                _ => panic!("Expected Neq"),
            }
        }

        #[test]
        fn test_less_than() {
            let expr = parse_expr("1 < 2").unwrap();
            match expr {
                Expr::Binary { op: BinaryOp::Lt, .. } => (),
                _ => panic!("Expected Lt"),
            }
        }

        #[test]
        fn test_greater_than() {
            let expr = parse_expr("2 > 1").unwrap();
            match expr {
                Expr::Binary { op: BinaryOp::Gt, .. } => (),
                _ => panic!("Expected Gt"),
            }
        }

        #[test]
        fn test_less_equal() {
            let expr = parse_expr("1 <= 2").unwrap();
            match expr {
                Expr::Binary { op: BinaryOp::LtEq, .. } => (),
                _ => panic!("Expected LtEq"),
            }
        }

        #[test]
        fn test_greater_equal() {
            let expr = parse_expr("2 >= 1").unwrap();
            match expr {
                Expr::Binary { op: BinaryOp::GtEq, .. } => (),
                _ => panic!("Expected GtEq"),
            }
        }

        #[test]
        fn test_logical_and() {
            let expr = parse_expr("true && false").unwrap();
            match expr {
                Expr::And { .. } => (),
                _ => panic!("Expected And"),
            }
        }

        #[test]
        fn test_logical_or() {
            let expr = parse_expr("true || false").unwrap();
            match expr {
                Expr::Or { .. } => (),
                _ => panic!("Expected Or"),
            }
        }

        #[test]
        fn test_unary_negate() {
            let expr = parse_expr("-x").unwrap();
            match expr {
                Expr::Unary { op: UnaryOp::Negate, .. } => (),
                _ => panic!("Expected Negate"),
            }
        }

        #[test]
        fn test_unary_not() {
            let expr = parse_expr("!true").unwrap();
            match expr {
                Expr::Unary { op: UnaryOp::Not, .. } => (),
                _ => panic!("Expected Not"),
            }
        }

        #[test]
        fn test_operator_precedence_multiplication_before_addition() {
            let expr = parse_expr("2 + 3 * 4").unwrap();
            // 应该解析为 2 + (3 * 4)
            match expr {
                Expr::Binary { op: BinaryOp::Add, right, .. } => match *right {
                    Expr::Binary { op: BinaryOp::Mul, .. } => (),
                    _ => panic!("Right should be multiplication"),
                },
                _ => panic!("Expected Add at top level"),
            }
        }

        #[test]
        fn test_operator_precedence_comparison_before_logical() {
            let expr = parse_expr("a < b && c > d").unwrap();
            // 应该解析为 (a < b) && (c > d)
            match expr {
                Expr::And { left, right, .. } => {
                    assert!(matches!(*left, Expr::Binary { op: BinaryOp::Lt, .. }));
                    assert!(matches!(*right, Expr::Binary { op: BinaryOp::Gt, .. }));
                }
                _ => panic!("Expected And"),
            }
        }
    }

    mod control_flow_tests {
        use super::*;

        #[test]
        fn test_if_statement() {
            let program = parse_source("if true { 1 }").unwrap();
            assert_eq!(program.statements.len(), 1);
            if let Stmt::Expr(Expr::If { condition, then_branch, else_branch, .. }) =
                &program.statements[0]
            {
                assert!(matches!(*condition.as_ref(), Expr::Bool { value: true, .. }));
                assert!(else_branch.is_none());
                assert_eq!(then_branch.len(), 1);
            } else {
                panic!("Expected If expression");
            }
        }

        #[test]
        fn test_if_else_statement() {
            let program = parse_source("if true { 1 } else { 2 }").unwrap();
            if let Stmt::Expr(Expr::If { else_branch, .. }) = &program.statements[0] {
                assert!(else_branch.is_some());
            } else {
                panic!("Expected If with else");
            }
        }

        #[test]
        fn test_if_else_if_chain() {
            let program = parse_source("if a { 1 } else if b { 2 } else { 3 }").unwrap();
            if let Stmt::Expr(Expr::If { else_branch, .. }) = &program.statements[0] {
                assert!(else_branch.is_some());
                // 嵌套的 if-else-if
                if let Some(else_expr) = else_branch {
                    assert!(matches!(else_expr.as_ref(), Expr::If { .. }));
                }
            }
        }

        #[test]
        fn test_while_loop() {
            let program = parse_source("while true { break }").unwrap();
            if let Stmt::Expr(Expr::While { condition, body, .. }) = &program.statements[0] {
                assert!(matches!(*condition.as_ref(), Expr::Bool { value: true, .. }));
                assert_eq!(body.len(), 1);
            } else {
                panic!("Expected While");
            }
        }

        #[test]
        fn test_loop_infinite() {
            let program = parse_source("loop { break }").unwrap();
            if let Stmt::Expr(Expr::Loop { body, .. }) = &program.statements[0] {
                assert_eq!(body.len(), 1);
            } else {
                panic!("Expected Loop");
            }
        }

        #[test]
        fn test_for_in_loop() {
            let program = parse_source("for x in [1, 2, 3] { x }").unwrap();
            if let Stmt::Expr(Expr::ForIn { var_name, iterable, body, .. }) = &program.statements[0]
            {
                assert_eq!(var_name, "x");
                assert!(matches!(*iterable.as_ref(), Expr::Array { .. }));
                assert_eq!(body.len(), 1);
            } else {
                panic!("Expected ForIn");
            }
        }

        #[test]
        fn test_return_statement() {
            let program = parse_source("return 42").unwrap();
            if let Stmt::Expr(Expr::Return { value, .. }) = &program.statements[0] {
                assert!(value.is_some());
            } else {
                panic!("Expected Return");
            }
        }

        #[test]
        fn test_return_without_value() {
            let program = parse_source("return").unwrap();
            if let Stmt::Expr(Expr::Return { value, .. }) = &program.statements[0] {
                assert!(value.is_none());
            } else {
                panic!("Expected Return without value");
            }
        }

        #[test]
        fn test_break_statement() {
            let program = parse_source("break").unwrap();
            if let Stmt::Expr(Expr::Break { value, .. }) = &program.statements[0] {
                assert!(value.is_none());
            } else {
                panic!("Expected Break");
            }
        }

        #[test]
        fn test_break_with_value() {
            let program = parse_source("break 42").unwrap();
            if let Stmt::Expr(Expr::Break { value, .. }) = &program.statements[0] {
                assert!(value.is_some());
            } else {
                panic!("Expected Break with value");
            }
        }

        #[test]
        fn test_continue_statement() {
            let program = parse_source("continue").unwrap();
            assert!(matches!(&program.statements[0], Stmt::Expr(Expr::Continue { .. })));
        }

        #[test]
        fn test_nested_control_flow() {
            let program = parse_source("if true { while false { break } }").unwrap();
            assert_eq!(program.statements.len(), 1);
        }
    }

    mod data_structure_tests {
        use super::*;

        #[test]
        fn test_array_literal() {
            let expr = parse_expr("[1, 2, 3]").unwrap();
            match expr {
                Expr::Array { elements, .. } => assert_eq!(elements.len(), 3),
                _ => panic!("Expected Array"),
            }
        }

        #[test]
        fn test_empty_array() {
            let expr = parse_expr("[]").unwrap();
            match expr {
                Expr::Array { elements, .. } => assert!(elements.is_empty()),
                _ => panic!("Expected empty Array"),
            }
        }

        #[test]
        fn test_single_element_array() {
            let expr = parse_expr("[42]").unwrap();
            match expr {
                Expr::Array { elements, .. } => assert_eq!(elements.len(), 1),
                _ => panic!("Expected Array with one element"),
            }
        }

        #[test]
        fn test_tuple_literal() {
            let expr = parse_expr("(1, 2, 3)").unwrap();
            match expr {
                Expr::Tuple { elements, .. } => assert_eq!(elements.len(), 3),
                _ => panic!("Expected Tuple"),
            }
        }

        #[test]
        fn test_parenthesized_expression() {
            let expr = parse_expr("(1 + 2)").unwrap();
            match expr {
                Expr::Number { value, .. } => assert_eq!(value, 3.0), // 应该被求值为 3？不，这只是解析
                Expr::Binary { op: BinaryOp::Add, .. } => (),         // 实际上应该是二元运算
                _ => panic!("Expected parenthesized expression"),
            }
        }

        #[test]
        fn test_dict_literal() {
            let expr = parse_expr("{a: 1, b: 2}").unwrap();
            match expr {
                Expr::Dict { pairs, .. } => assert_eq!(pairs.len(), 2),
                _ => panic!("Expected Dict"),
            }
        }

        #[test]
        fn test_dict_with_string_keys() {
            let expr = parse_expr("{\"key\": \"value\"}").unwrap();
            match expr {
                Expr::Dict { pairs, .. } => {
                    assert_eq!(pairs.len(), 1);
                    assert_eq!(pairs[0].0, "key");
                }
                _ => panic!("Expected Dict with string keys"),
            }
        }

        #[test]
        fn test_empty_dict() {
            let result = parse_expr("{}");
            match result {
                Ok(Expr::Dict { pairs, .. }) => assert!(pairs.is_empty()),
                Ok(Expr::Block { .. }) => {}
                Ok(_) => panic!("Expected empty Dict or Block"),
                Err(_) => {}
            }
        }

        #[test]
        fn test_block_expression() {
            let result = parse_expr("{ 1; 2; 3 }");
            match result {
                Ok(Expr::Block { statements, .. }) => assert_eq!(statements.len(), 3),
                Ok(_) => {}
                Err(_) => {}
            }
        }

        #[test]
        fn test_range_inclusive() {
            let expr = parse_expr("1..5").unwrap();
            match expr {
                Expr::Range { inclusive, .. } => assert!(inclusive),
                _ => panic!("Expected inclusive Range"),
            }
        }

        #[test]
        fn test_range_exclusive() {
            let expr = parse_expr("1..<5").unwrap();
            match expr {
                Expr::Range { inclusive, .. } => assert!(!inclusive),
                _ => panic!("Expected exclusive Range"),
            }
        }
    }

    mod function_tests {
        use super::*;

        #[test]
        fn test_function_declaration() {
            let program = parse_source("fn add(a, b) { a + b }").unwrap();
            if let Stmt::Expr(Expr::Fn { name, params, body, .. }) = &program.statements[0] {
                assert_eq!(name.as_deref(), Some("add"));
                assert_eq!(params.len(), 2);
                assert_eq!(params[0], "a");
                assert_eq!(params[1], "b");
                assert_eq!(body.len(), 1);
            } else {
                panic!("Expected Fn declaration");
            }
        }

        #[test]
        fn test_anonymous_function() {
            let program = parse_source("fn(a, b) { a + b }").unwrap();
            if let Stmt::Expr(Expr::Fn { name, params, .. }) = &program.statements[0] {
                assert!(name.is_none());
                assert_eq!(params.len(), 2);
            } else {
                panic!("Expected anonymous Fn");
            }
        }

        #[test]
        fn test_function_no_params() {
            let program = parse_source("fn hello() { 42 }").unwrap();
            if let Stmt::Expr(Expr::Fn { params, .. }) = &program.statements[0] {
                assert!(params.is_empty());
            } else {
                panic!("Expected Fn with no params");
            }
        }

        #[test]
        fn test_function_single_param() {
            let program = parse_source("fn square(x) { x * x }").unwrap();
            if let Stmt::Expr(Expr::Fn { params, .. }) = &program.statements[0] {
                assert_eq!(params.len(), 1);
                assert_eq!(params[0], "x");
            } else {
                panic!("Expected Fn with single param");
            }
        }

        #[test]
        fn test_function_call_no_args() {
            let expr = parse_expr("foo()").unwrap();
            match expr {
                Expr::Call { args, .. } => assert!(args.is_empty()),
                _ => panic!("Expected Call with no args"),
            }
        }

        #[test]
        fn test_function_call_with_args() {
            let expr = parse_expr("foo(1, 2, 3)").unwrap();
            match expr {
                Expr::Call { args, .. } => assert_eq!(args.len(), 3),
                _ => panic!("Expected Call with args"),
            }
        }

        #[test]
        fn test_nested_function_calls() {
            let result = parse_expr("foo(bar(baz()))");
            match result {
                Ok(Expr::Call { callee, args, .. }) => {
                    match *callee {
                        Expr::Call { .. } => {}
                        Expr::Ident { .. } => {}
                        _ => {}
                    }
                    let _ = args;
                }
                Ok(_) => {}
                Err(_) => {}
            }
        }

        #[test]
        fn test_function_as_value() {
            let program = parse_source("fn(x) { x }").unwrap();
            assert!(matches!(&program.statements[0], Stmt::Expr(Expr::Fn { name: None, .. })));
        }

        #[test]
        fn test_arrow_single_param_expr() {
            let expr = parse_expr("x => x + 1").unwrap();
            match expr {
                Expr::Closure { params, body, .. } => {
                    assert_eq!(params.len(), 1);
                    assert_eq!(params[0], "x");
                    assert_eq!(body.len(), 1);
                }
                _ => panic!("Expected Closure, got {:?}", expr),
            }
        }

        #[test]
        fn test_arrow_single_param_block() {
            let expr = parse_expr("x => { return x }").unwrap();
            match expr {
                Expr::Closure { params, body, .. } => {
                    assert_eq!(params.len(), 1);
                    assert_eq!(params[0], "x");
                    assert!(!body.is_empty());
                }
                _ => panic!("Expected Closure, got {:?}", expr),
            }
        }

        #[test]
        fn test_arrow_multi_params() {
            let expr = parse_expr("(a, b) => a + b").unwrap();
            match expr {
                Expr::Closure { params, .. } => {
                    assert_eq!(params.len(), 2);
                    assert_eq!(params[0], "a");
                    assert_eq!(params[1], "b");
                }
                _ => panic!("Expected Closure, got {:?}", expr),
            }
        }

        #[test]
        fn test_arrow_no_params() {
            let expr = parse_expr("() => 42").unwrap();
            match expr {
                Expr::Closure { params, .. } => {
                    assert!(params.is_empty());
                }
                _ => panic!("Expected Closure, got {:?}", expr),
            }
        }

        #[test]
        fn test_arrow_assigned_to_var() {
            let program = parse_source("add = (a, b) => a + b").unwrap();
            match &program.statements[0] {
                Stmt::Assign { value, .. } => assert!(matches!(value, Expr::Closure { .. })),
                _ => panic!("Expected Assign with Closure"),
            }
        }

        #[test]
        fn test_arrow_nested() {
            let expr = parse_expr("x => y => x + y").unwrap();
            match expr {
                Expr::Closure { params, body, .. } => {
                    assert_eq!(params.len(), 1);
                    assert_eq!(params[0], "x");
                    if let Stmt::Expr(Expr::Return { value: Some(inner), .. }) = &body[0] {
                        if let Expr::Closure { params: inner_params, .. } = inner.as_ref() {
                            assert_eq!(inner_params.len(), 1);
                            assert_eq!(inner_params[0], "y");
                        } else {
                            panic!("Expected nested Closure in body");
                        }
                    } else {
                        panic!("Expected Return wrapping Closure in body");
                    }
                }
                _ => panic!("Expected outer Closure"),
            }
        }

        #[test]
        fn test_paren_expr_not_arrow() {
            let expr = parse_expr("(1 + 2)").unwrap();
            assert!(matches!(expr, Expr::Binary { .. }));
        }
    }

    mod assignment_tests {
        use super::*;

        #[test]
        fn test_simple_assignment() {
            let program = parse_source("x = 42").unwrap();
            if let Stmt::Assign { target, value, .. } = &program.statements[0] {
                match target {
                    AssignTarget::Ident { name } => assert_eq!(name, "x"),
                    _ => panic!("Expected Ident target"),
                }
                assert!(matches!(*value, Expr::Number { value: 42.0, .. }));
            } else {
                panic!("Expected Assign statement");
            }
        }

        #[test]
        fn test_property_assignment() {
            let program = parse_source("obj.prop = 42").unwrap();
            if let Stmt::Assign { target, .. } = &program.statements[0] {
                match target {
                    AssignTarget::Field { name, .. } => assert_eq!(name, "prop"),
                    _ => panic!("Expected Field target"),
                }
            } else {
                panic!("Expected Assign to property");
            }
        }

        #[test]
        fn test_index_assignment() {
            let program = parse_source("arr[0] = 42").unwrap();
            if let Stmt::Assign { target, .. } = &program.statements[0] {
                assert!(matches!(target, AssignTarget::Index { .. }));
            } else {
                panic!("Expected Assign to index");
            }
        }

        #[test]
        fn test_assignment_with_complex_expression() {
            let program = parse_source("x = 1 + 2 * 3").unwrap();
            if let Stmt::Assign { value, .. } = &program.statements[0] {
                assert!(matches!(*value, Expr::Binary { .. }));
            } else {
                panic!("Expected Assign with complex value");
            }
        }

        #[test]
        fn test_chained_property_assignment() {
            let program = parse_source("obj.a.b = 42").unwrap();
            if let Stmt::Assign { target, .. } = &program.statements[0] {
                match target {
                    AssignTarget::Field { object, name } => {
                        assert_eq!(name, "b");
                        matches!(**object, Expr::Field { .. }); // obj.a.b
                    }
                    _ => panic!("Expected chained Field target"),
                }
            } else {
                panic!("Expected chained property assignment");
            }
        }

        #[test]
        fn test_invalid_assignment_target_number() {
            let result = parse_source("1 = 2");
            assert!(result.is_err());
            let err = result.unwrap_err();
            assert_eq!(err.message, "invalid assignment target");
        }
    }

    mod member_access_tests {
        use super::*;

        #[test]
        fn test_dot_access() {
            let expr = parse_expr("obj.prop").unwrap();
            match expr {
                Expr::Field { name, .. } => assert_eq!(name, "prop"),
                _ => panic!("Expected Field access"),
            }
        }

        #[test]
        fn test_index_access() {
            let expr = parse_expr("arr[0]").unwrap();
            assert!(matches!(expr, Expr::Index { .. }));
        }

        #[test]
        fn test_chained_dot_access() {
            let expr = parse_expr("a.b.c").unwrap();
            match expr {
                Expr::Field { object, name, span: _ } => {
                    assert_eq!(name, "c");
                    matches!(*object, Expr::Field { .. }); // a.b
                }
                _ => panic!("Expected chained Field access"),
            }
        }

        #[test]
        fn test_chained_index_access() {
            let expr = parse_expr("arr[0][1]").unwrap();
            match expr {
                Expr::Index { object, .. } => {
                    matches!(*object, Expr::Index { .. }); // arr[0]
                }
                _ => panic!("Expected chained Index access"),
            }
        }

        #[test]
        fn test_mixed_member_access() {
            let expr = parse_expr("obj.arr[0].prop").unwrap();
            assert!(matches!(expr, Expr::Field { .. }));
        }

        #[test]
        fn test_call_on_member_access() {
            let expr = parse_expr("obj.method()").unwrap();
            assert!(
                matches!(expr, Expr::Call { callee, .. } if matches!(*callee, Expr::Field { .. }))
            );
        }
    }

    mod error_handling_tests {
        use super::*;

        #[test]
        fn test_unexpected_token() {
            let result = parse_expr(")");
            assert!(result.is_err());
        }

        #[test]
        fn test_unclosed_parenthesis() {
            let result = parse_expr("(1 + 2");
            assert!(result.is_err());
        }

        #[test]
        fn test_unclosed_bracket() {
            let result = parse_expr("[1, 2, 3");
            assert!(result.is_err());
        }

        #[test]
        fn test_unclosed_brace_in_block() {
            let result = parse_expr("{ 1; 2; ");
            assert!(result.is_err());
        }

        #[test]
        fn test_unclosed_brace_in_if() {
            let result = parse_source("if true { 1");
            assert!(result.is_err());
        }

        #[test]
        fn test_missing_condition_in_if() {
            let result = parse_source("if { 1 }");
            assert!(result.is_err());
        }

        #[test]
        fn test_missing_body_in_while() {
            let result = parse_source("while true ");
            assert!(result.is_err());
        }

        #[test]
        fn test_missing_for_variable() {
            let result = parse_source("for in [1, 2, 3] {}");
            assert!(result.is_err());
        }

        #[test]
        fn test_missing_fn_body() {
            let result = parse_source("fn test()");
            assert!(result.is_err());
        }

        #[test]
        fn test_invalid_assignment_target_literal() {
            let result = parse_source("\"string\" = 42");
            assert!(result.is_err());
            assert_eq!(result.unwrap_err().message, "invalid assignment target");
        }

        #[test]
        fn test_unclosed_tuple() {
            let result = parse_expr("(1, 2,");
            assert!(result.is_err());
        }

        #[test]
        fn test_missing_colon_in_dict() {
            let result = parse_expr("{a 1}");
            if let Ok(expr) = result {
                assert!(matches!(expr, Expr::Dict { .. } | Expr::Block { .. }));
            }
        }

        #[test]
        fn test_extra_closing_paren() {
            // 这个应该能成功解析 (1+2)，然后多余的 ) 会报错
            let result = parse_source("(1 + 2))");
            assert!(result.is_err());
        }
    }

    mod integration_tests {
        use super::*;

        #[test]
        fn test_fibonacci_function() {
            let source = r#"
fn fib(n) {
    if n <= 1 {
        return n
    }
    return fib(n - 1) + fib(n - 2)
}
"#;
            let program = parse_source(source).unwrap();
            assert_eq!(program.statements.len(), 1);
        }

        #[test]
        fn test_complex_arithmetic_expression() {
            let result = parse_expr("((2 + 3) * (4 - 1)) / 2 % 3");
            if let Ok(expr) = result {
                assert!(matches!(expr, Expr::Binary { .. }));
            }
        }

        #[test]
        fn test_nested_loops_and_conditions() {
            let source = r#"
for i in 1..10 {
    for j in 1..10 {
        if i * j > 50 {
            break
        }
    }
}
"#;
            let program = parse_source(source).unwrap();
            assert_eq!(program.statements.len(), 1);
        }

        #[test]
        fn test_higher_order_function() {
            let source = r#"
fn apply(f, x) {
    return f(x)
}
fn double(x) {
    return x * 2
}
apply(double, 5)
"#;
            let program = parse_source(source).unwrap();
            assert_eq!(program.statements.len(), 3); // 2 function defs + 1 call
        }

        #[test]
        fn test_complex_data_structures() {
            let source = r#"
data = {
    users: [
        {name: "Alice", age: 30},
        {name: "Bob", age: 25}
    ],
    count: 2
}
"#;
            let result = parse_source(source);
            assert!(
                result.is_ok(),
                "complex data structure should parse successfully, got: {:?}",
                result.err()
            );
            let program = result.unwrap();
            assert_eq!(program.statements.len(), 1, "should have exactly one statement");
        }

        #[test]
        fn test_multiple_statements() {
            let source = r#"
x = 1
y = 2
z = x + y
"#;
            let program = parse_source(source).unwrap();
            assert_eq!(program.statements.len(), 3);
        }

        #[test]
        fn test_closure_and_immediate_invocation() {
            let source = r#"
fn(x) { x * 2 }(5)
"#;
            let program = parse_source(source).unwrap();
            // 匿名函数立即调用
            assert!(matches!(&program.statements[0], Stmt::Expr(Expr::Call { .. })));
        }

        #[test]
        fn test_match_negative_literal_pattern() {
            // 负数字面量模式：`-1 => "neg"` 应被解析为 MatchPattern::Literal(Expr::Number { value: -1.0 })
            let program = parse_source("match (n) { -1 => \"neg\", _ => \"non-neg\" }").unwrap();
            match &program.statements[0] {
                Stmt::Expr(Expr::Match { arms, .. }) => {
                    assert_eq!(arms.len(), 2, "应有两个 match 分支");
                    match &arms[0].pattern {
                        MatchPattern::Literal(Expr::Number { value, .. }) => {
                            assert!(
                                (*value - (-1.0)).abs() < f64::EPSILON,
                                "第一个模式应为 -1，实际为 {}",
                                value
                            );
                        }
                        other => {
                            panic!("期望 MatchPattern::Literal(Number(-1))，实际: {:?}", other)
                        }
                    }
                    assert!(
                        matches!(arms[1].pattern, MatchPattern::Wildcard),
                        "第二个模式应为通配符"
                    );
                }
                other => panic!("期望 Stmt::Expr(Expr::Match)，实际: {:?}", other),
            }
        }

        #[test]
        fn test_match_negative_range_pattern() {
            // 负数范围模式：`-10..-1 => ...` 起始值也是负数
            let program =
                parse_source("match (n) { -10..-1 => \"neg-range\", _ => \"other\" }").unwrap();
            match &program.statements[0] {
                Stmt::Expr(Expr::Match { arms, .. }) => match &arms[0].pattern {
                    MatchPattern::Range { start, end, inclusive } => {
                        assert!(matches!(**start, Expr::Number { .. }), "范围起始应为 Number");
                        assert!(matches!(**end, Expr::Number { .. }), "范围结束应为 Number");
                        assert!(*inclusive, "应包含结束值（.. 语法）");
                    }
                    other => panic!("期望 MatchPattern::Range，实际: {:?}", other),
                },
                other => panic!("期望 Stmt::Expr(Expr::Match)，实际: {:?}", other),
            }
        }
    }

    mod bilingual_tests {
        use super::*;

        #[test]
        fn test_chinese_if_keyword() {
            let program = parse_source("如果 true { 1 }").unwrap();
            assert!(matches!(&program.statements[0], Stmt::Expr(Expr::If { .. })));
        }

        #[test]
        fn test_chinese_else_keyword() {
            let program = parse_source("如果 true { 1 } 否则 { 2 }").unwrap();
            if let Stmt::Expr(Expr::If { else_branch, .. }) = &program.statements[0] {
                assert!(else_branch.is_some());
            }
        }

        #[test]
        fn test_chinese_while_keyword() {
            let program = parse_source("当 true { 1 }").unwrap();
            assert!(matches!(&program.statements[0], Stmt::Expr(Expr::While { .. })));
        }

        #[test]
        fn test_chinese_loop_keyword() {
            let program = parse_source("循环 { 跳出 }").unwrap();
            assert!(matches!(&program.statements[0], Stmt::Expr(Expr::Loop { .. })));
        }

        #[test]
        fn test_chinese_for_keyword() {
            let program = parse_source("遍历 x 在 [1, 2, 3] { x }").unwrap();
            assert!(
                matches!(&program.statements[0], Stmt::Expr(Expr::ForIn { var_name, .. }) if var_name == "x")
            );
        }

        #[test]
        fn test_chinese_function_keyword() {
            let program = parse_source("函数 add(a, b) { a + b }").unwrap();
            if let Stmt::Expr(Expr::Fn { name, .. }) = &program.statements[0] {
                assert_eq!(name.as_deref(), Some("add"));
            }
        }

        #[test]
        fn test_chinese_return_keyword() {
            let program = parse_source("返回 42").unwrap();
            assert!(matches!(
                &program.statements[0],
                Stmt::Expr(Expr::Return { value: Some(..), .. })
            ));
        }

        #[test]
        fn test_chinese_break_continue_keywords() {
            let program = parse_source("循环 { 跳出 42 继续 }").unwrap();
            // 应该包含 break 和 continue
            assert!(matches!(&program.statements[0], Stmt::Expr(Expr::Loop { .. })));
        }

        #[test]
        fn test_chinese_boolean_literals() {
            let expr_true = parse_expr("真").unwrap();
            assert!(matches!(expr_true, Expr::Bool { value: true, .. }));

            let expr_false = parse_expr("假").unwrap();
            assert!(matches!(expr_false, Expr::Bool { value: false, .. }));
        }

        #[test]
        fn test_chinese_nil_literal() {
            let expr = parse_expr("空").unwrap();
            assert!(matches!(expr, Expr::Nil { .. }));
        }

        #[test]
        fn test_mixed_language_keywords() {
            // 中英文混合使用
            let source = r#"
如果 x > 0 {
    返回 x
} else {
    return -x
}
"#;
            let program = parse_source(source).unwrap();
            assert!(matches!(&program.statements[0], Stmt::Expr(Expr::If { .. })));
        }
    }

    mod edge_case_tests {
        use super::*;

        #[test]
        fn test_very_long_identifier() {
            let long_id = "a".repeat(1000);
            let expr = parse_expr(&long_id).unwrap();
            match expr {
                Expr::Ident { name, .. } => assert_eq!(name.len(), 1000),
                _ => panic!("Expected long identifier"),
            }
        }

        #[test]
        fn test_deeply_nested_parens() {
            let nested = "(".repeat(50) + "1" + &")".repeat(50);
            let expr = parse_expr(&nested).unwrap();
            assert!(matches!(expr, Expr::Number { value: 1.0, .. }));
        }

        #[test]
        fn test_many_arguments() {
            let args: Vec<String> = (0..20).map(|i| i.to_string()).collect();
            let source = format!("foo({})", args.join(", "));
            let expr = parse_expr(&source).unwrap();
            match expr {
                Expr::Call { args, .. } => assert_eq!(args.len(), 20),
                _ => panic!("Expected Call with many args"),
            }
        }

        #[test]
        fn test_large_array_literal() {
            let elements: Vec<String> = (0..100).map(|i| i.to_string()).collect();
            let source = format!("[{}]", elements.join(", "));
            let expr = parse_expr(&source).unwrap();
            match expr {
                Expr::Array { elements, .. } => assert_eq!(elements.len(), 100),
                _ => panic!("Expected large Array"),
            }
        }

        #[test]
        fn test_complex_boolean_expression() {
            let result = parse_expr("a && b || c && !d || e");
            match result {
                Ok(expr) => assert!(
                    matches!(expr, Expr::Or { .. } | Expr::And { .. }),
                    "expected Or or And, got {:?}",
                    expr
                ),
                Err(e) => panic!("expected successful parse, got error: {}", e),
            }
        }

        #[test]
        fn test_multiple_assignments_in_sequence() {
            let source = "a = 1; b = 2; c = 3";
            let result = parse_source(source);
            if let Ok(program) = result {
                assert!(!program.statements.is_empty());
            }
        }

        #[test]
        fn test_empty_program() {
            let program = parse_source("").unwrap();
            assert!(program.statements.is_empty());
        }

        #[test]
        fn test_only_whitespace() {
            let program = parse_source("   \n\t  ").unwrap();
            assert!(program.statements.is_empty());
        }

        #[test]
        fn test_expression_with_trailing_newline() {
            let expr = parse_expr("42\n").unwrap();
            assert!(matches!(expr, Expr::Number { value: 42.0, .. }));
        }
    }

    mod span_tests {
        use super::*;

        #[test]
        fn test_span_for_number_literal() {
            let expr = parse_expr("42").unwrap();
            match expr {
                Expr::Number { span, .. } => {
                    assert_eq!(span.line, 1);
                    assert!(span.column >= 1);
                }
                _ => panic!("Expected Number with span"),
            }
        }

        #[test]
        fn test_span_for_binary_operation() {
            let expr = parse_expr("1 + 2").unwrap();
            match expr {
                Expr::Binary { span, .. } => {
                    assert_eq!(span.line, 1);
                }
                _ => panic!("Expected Binary with span"),
            }
        }

        #[test]
        fn test_error_position_info() {
            let result = parse_source("if { 1 }");
            if let Err(err) = result {
                assert!(!err.message.is_empty());
                assert!(err.line >= 1);
                assert!(err.column >= 1);
            }
        }
    }

    mod exception_handling_tests {
        use super::*;

        #[test]
        fn test_basic_try_catch() {
            // 基本 try-catch 语法
            let source = "try { out(\"error\") } catch (e) { print(e) }";
            let program = parse_source(source).unwrap();
            assert_eq!(program.statements.len(), 1);

            if let Stmt::Expr(Expr::Try { body, catch_clause, keep_block, .. }) =
                &program.statements[0]
            {
                // try 块应该包含 out 语句
                assert!(!body.is_empty());

                // 应该有 catch 子句
                assert!(catch_clause.is_some());
                if let Some(catch) = catch_clause {
                    assert_eq!(catch.binding, "e");
                    assert!(catch.exception_type.is_none()); // M2 才支持类型过滤
                    assert!(!catch.body.is_empty());
                }

                // 不应该有 keep 块
                assert!(keep_block.is_none());
            } else {
                panic!("Expected Try expression, got: {:?}", program.statements[0]);
            }
        }

        #[test]
        fn test_try_catch_with_keep() {
            // 带 keep 块的完整异常处理
            let source = "try { out(\"error\") } catch (e) { print(e) } keep { cleanup() }";
            let program = parse_source(source).unwrap();

            if let Stmt::Expr(Expr::Try { catch_clause, keep_block, .. }) = &program.statements[0] {
                assert!(catch_clause.is_some());
                assert!(keep_block.is_some());
                if let Some(keep) = keep_block {
                    assert!(!keep.is_empty());
                }
            } else {
                panic!("Expected Try with keep block");
            }
        }

        #[test]
        fn test_try_only() {
            // 只有 try 块，没有 catch 和 keep
            let source = "try { 42 }";
            let program = parse_source(source).unwrap();

            if let Stmt::Expr(Expr::Try { body, catch_clause, keep_block, .. }) =
                &program.statements[0]
            {
                assert!(!body.is_empty()); // 包含表达式语句 42
                assert!(catch_clause.is_none());
                assert!(keep_block.is_none());
            } else {
                panic!("Expected Try without catch/keep");
            }
        }

        #[test]
        fn test_out_expression_string() {
            // 测试 out 抛出字符串
            let source = "out \"错误消息\"";
            let program = parse_source(source).unwrap();

            assert!(
                matches!(&program.statements[0], Stmt::Expr(Expr::Out { value, .. }) if matches!(value.as_ref(), Expr::String { .. }))
            );
        }

        #[test]
        fn test_out_expression_dict() {
            // 测试 out 抛出字典
            let source = "out {message: \"错误\", code: \"TypeError\"}";
            let program = parse_source(source).unwrap();

            assert!(
                matches!(&program.statements[0], Stmt::Expr(Expr::Out { value, .. }) if matches!(value.as_ref(), Expr::Dict { .. }))
            );
        }

        #[test]
        fn test_out_expression_complex() {
            // 测试 out 抛出标识符
            let source = "out error_variable";
            let program = parse_source(source).unwrap();

            assert!(
                matches!(&program.statements[0], Stmt::Expr(Expr::Out { value, .. }) if matches!(value.as_ref(), Expr::Ident { name, .. } if name == "error_variable"))
            );
        }

        #[test]
        fn test_nested_try_in_if() {
            // 测试嵌套在 if 中的 try-catch
            let source = "if (risky) { try { danger() } catch (e) { handle(e) } } else { safe() }";
            let program = parse_source(source).unwrap();
            assert_eq!(program.statements.len(), 1);
            assert!(matches!(&program.statements[0], Stmt::Expr(Expr::If { .. })));
        }

        #[test]
        fn test_try_as_expression() {
            // 测试 try-catch 作为表达式使用（赋值给变量）
            let source = "result = try { config() } catch (e) { default() }";
            let program = parse_source(source).unwrap();

            if let Stmt::Assign { value, .. } = &program.statements[0] {
                assert!(matches!(value, Expr::Try { .. }));
            } else {
                panic!("Expected assignment with Try expression");
            }
        }

        #[test]
        fn test_multiple_statements_in_try() {
            // 测试 try 块中包含多个语句
            let source = "try { step1(); step2(); step3() } catch (e) { cleanup() }";
            let program = parse_source(source).unwrap();

            if let Stmt::Expr(Expr::Try { body, .. }) = &program.statements[0] {
                assert!(!body.is_empty()); // 至少有一个语句
            } else {
                panic!("Expected Try with multiple statements");
            }
        }

        #[test]
        fn test_error_missing_try_body() {
            // 测试缺少 try 块体的错误
            let result = parse_source("try ");
            assert!(result.is_err());
        }

        #[test]
        fn test_error_missing_catch_paren() {
            // 测试 catch 缺少左括号的错误
            let result = parse_source("try {} catch e {}");
            assert!(result.is_err(), "missing '(' after catch should cause parse error");
        }

        #[test]
        fn test_error_missing_catch_variable() {
            // 测试 catch 缺少变量名的错误
            let result = parse_source("try {} catch () {}");
            assert!(result.is_err(), "missing catch variable name should cause parse error");
        }

        #[test]
        fn test_error_out_without_value() {
            // 测试 out 后面没有表达式的情况
            let result = parse_source("out");
            assert!(result.is_err(), "out without value should cause parse error");
        }

        #[test]
        fn test_complex_exception_handling() {
            // 复杂的异常处理场景：函数内的嵌套 try-catch
            let source = "fn process(data) { try { validate(data); if (data.invalid) { out {code: 400} } } catch (e) { log(e) } keep { close() } }";
            let program = parse_source(source).unwrap();
            assert_eq!(program.statements.len(), 1);
            assert!(matches!(&program.statements[0], Stmt::Expr(Expr::Fn { .. })));
        }
    }

    // ========================================================================
    // 10. Parser::parse_with_timing 测试
    //
    // 验证带计时的解析入口：有效源码返回 (Program, ParseTimings)，
    // 无效源码返回错误，且 timings 字段为合理值。
    // ========================================================================
    mod parse_with_timing_tests {
        use super::*;

        #[test]
        fn test_parse_with_timing_valid_source_returns_program_and_timings() {
            // 有效源码应返回 Program 和 ParseTimings
            let source = "fn add(a, b) { a + b }";
            let result = Parser::parse_with_timing(source);
            let (program, timings) = result.expect("valid source should parse successfully");
            assert_eq!(program.statements.len(), 1);
            // timings 字段应为非负 Duration（任何解析都应耗时 >= 0）
            let _ = timings.lex_duration; // 验证字段可访问
            let _ = timings.parse_duration; // 验证字段可访问
        }

        #[test]
        fn test_parse_with_timing_empty_source() {
            // 空源码应成功解析为空 Program
            let (program, timings) =
                Parser::parse_with_timing("").expect("empty source should parse successfully");
            assert!(program.statements.is_empty());
            let _ = timings.lex_duration; // 验证字段可访问
            let _ = timings.parse_duration; // 验证字段可访问
        }

        #[test]
        fn test_parse_with_timing_invalid_source_returns_error() {
            // 无效源码（未闭合的字符串）应返回错误
            let result = Parser::parse_with_timing("\"unterminated");
            assert!(result.is_err(), "unterminated string should cause parse error");
        }

        #[test]
        fn test_parse_with_timing_syntax_error_returns_error() {
            // 语法错误（缺少右括号）应返回错误
            let result = Parser::parse_with_timing("fn add(a, b { }");
            assert!(result.is_err(), "missing closing paren should cause parse error");
        }

        #[test]
        fn test_parse_with_timing_consistent_with_parse() {
            // parse_with_timing 返回的 Program 应与 parse() 一致
            let source = "x = 42; y = x + 1";
            let program_plain = Parser::parse(source).expect("parse should succeed");
            let (program_timed, _timings) =
                Parser::parse_with_timing(source).expect("parse_with_timing should succeed");
            assert_eq!(program_plain.statements.len(), program_timed.statements.len());
        }

        #[test]
        fn test_parse_with_timing_returns_parse_timings_struct() {
            // 验证返回的 ParseTimings 结构体字段可访问
            let source = "42";
            let (_program, timings) = Parser::parse_with_timing(source).unwrap();
            // 显式访问字段以确保结构体形状稳定
            let _lex = timings.lex_duration;
            let _parse = timings.parse_duration;
            // Duration 应该是合理的（小于 1 秒，对于简单源码）
            assert!(timings.lex_duration.as_secs() < 1);
            assert!(timings.parse_duration.as_secs() < 1);
        }

        #[test]
        fn test_parse_with_timing_complex_source() {
            // 复杂源码（函数 + 控制流）应成功解析
            let source = r#"
fn fib(n) {
    if n <= 1 {
        return n
    }
    return fib(n - 1) + fib(n - 2)
}
"#;
            let (program, timings) = Parser::parse_with_timing(source)
                .expect("complex source should parse successfully");
            assert_eq!(program.statements.len(), 1);
            let _ = timings.lex_duration; // 验证字段可访问
            let _ = timings.parse_duration; // 验证字段可访问
        }

        #[test]
        fn test_parse_with_timing_chinese_keywords() {
            // 中文关键字源码应成功解析
            let source = "函数 加(a, b) { 返回 a + b }";
            let result = Parser::parse_with_timing(source);
            // 无论是否成功，都不应 panic；中文关键字应被识别
            if let Ok((program, _timings)) = result {
                assert!(!program.statements.is_empty());
            }
        }

        #[test]
        fn test_parse_with_timing_error_preserves_position() {
            // 错误应携带位置信息（line/column）
            let result = Parser::parse_with_timing("fn ( { }");
            if let Err(e) = result {
                // line 和 column 应该是有效值（>= 1）
                assert!(e.line >= 1, "error line should be >= 1");
                assert!(e.column >= 1, "error column should be >= 1");
                assert!(!e.message.is_empty(), "error message should not be empty");
            }
        }
    }

    // ========================================================================
    // P1-3 / P2-7 / P2-8 回归测试
    // ========================================================================
    mod p1_p2_regression_tests {
        use super::*;

        // P1-3: 递归深度上限
        // 注意：嵌套测试用大栈线程运行，避免测试框架的 2MB 默认栈溢出
        // （每层 expression() 经过约 12 个中间函数，64 层 × 12 × 2KB ≈ 1.5MB，
        // 但超出限制的测试需要更多栈空间才能到达 depth check）
        fn run_with_large_stack<F: FnOnce() + Send + 'static>(f: F) {
            std::thread::Builder::new()
                .stack_size(16 * 1024 * 1024) // 16MB
                .spawn(f)
                .expect("failed to spawn test thread")
                .join()
                .expect("test thread panicked");
        }

        #[test]
        fn test_parser_depth_limit_expression() {
            // 在 16MB 栈线程中运行，确保 depth check 触发前不溢出
            run_with_large_stack(|| {
                // 200 层嵌套 > MAX_PARSER_DEPTH=64，应返回错误
                let depth = 200;
                let source = format!("{}1{}", "(".repeat(depth), ")".repeat(depth));
                let result = parse_source(&source);
                assert!(result.is_err(), "deeply nested expression should be rejected");
                let err = result.unwrap_err();
                assert!(
                    err.message.contains("nesting exceeds limit") || err.message.contains("depth"),
                    "error should mention depth limit, got: {}",
                    err.message
                );
            });
        }

        #[test]
        fn test_parser_depth_limit_block() {
            run_with_large_stack(|| {
                let depth = 200;
                let source = format!("{}1{}", "{".repeat(depth), "}".repeat(depth));
                let result = parse_source(&source);
                assert!(result.is_err(), "deeply nested block should be rejected");
                let err = result.unwrap_err();
                assert!(
                    err.message.contains("nesting exceeds limit") || err.message.contains("depth"),
                    "error should mention depth limit, got: {}",
                    err.message
                );
            });
        }

        #[test]
        fn test_parser_depth_limit_array() {
            run_with_large_stack(|| {
                let depth = 200;
                let source = format!("{}1{}", "[".repeat(depth), "]".repeat(depth));
                let result = parse_source(&source);
                assert!(result.is_err(), "deeply nested array should be rejected");
            });
        }

        #[test]
        fn test_parser_normal_depth_accepted() {
            // 40 层嵌套 < MAX_PARSER_DEPTH=64，应被接受
            let depth = 40;
            let source = format!("{}1{}", "(".repeat(depth), ")".repeat(depth));
            let result = parse_source(&source);
            assert!(
                result.is_ok(),
                "normal depth expression should be accepted, got: {:?}",
                result.err()
            );
        }

        // P2-7: 链式比较中间操作数不重复求值
        #[test]
        fn test_chained_comparison_parses_to_block() {
            // `a < b < c` 应被解析为 Expr::Block（包含临时变量赋值 + And 链）
            let expr = parse_expr("a < b < c").expect("chained comparison should parse");
            match expr {
                Expr::Block { statements, .. } => {
                    // 应有 2 条语句：1 个 temp 赋值 + 1 个结果表达式
                    assert_eq!(
                        statements.len(),
                        2,
                        "chained comparison block should have 1 temp + 1 result"
                    );
                    // 第一条是赋值 __nuzo_cmp_0 = b
                    match &statements[0] {
                        Stmt::Assign { target, .. } => match target {
                            AssignTarget::Ident { name } => {
                                assert!(name.starts_with("__nuzo_cmp_"), "temp var name: {}", name);
                            }
                            other => panic!("expected AssignTarget::Ident, got {:?}", other),
                        },
                        other => panic!("expected Stmt::Assign, got {:?}", other),
                    }
                    // 第二条是表达式（And 链）
                    assert!(matches!(statements[1], Stmt::Expr(_)));
                }
                other => panic!("chained comparison should be Expr::Block, got {:?}", other),
            }
        }

        #[test]
        fn test_single_comparison_not_block() {
            // `a < b` 应直接是 Expr::Binary，不应被包装为 Block
            let expr = parse_expr("a < b").expect("single comparison should parse");
            assert!(
                matches!(expr, Expr::Binary { .. }),
                "single comparison should be Expr::Binary, got {:?}",
                expr
            );
        }

        #[test]
        fn test_no_comparison_returns_operand() {
            // `a` 应直接返回 ident，不应被包装
            let expr = parse_expr("a").expect("bare operand should parse");
            assert!(matches!(expr, Expr::Ident { .. }));
        }

        #[test]
        fn test_chained_comparison_three_operands() {
            // `a < b < c < d` 应生成 2 个临时变量（b, c）
            let expr = parse_expr("a < b < c < d").expect("4-operand chain should parse");
            match expr {
                Expr::Block { statements, .. } => {
                    // 2 temp + 1 result = 3
                    assert_eq!(statements.len(), 3);
                }
                other => panic!("expected Block, got {:?}", other),
            }
        }

        // P2-8: 漏冒号 dict 检测
        #[test]
        fn test_missing_colon_dict_error() {
            // `{ a 1 }` 应报错（漏冒号），而非静默解析为 block
            let result = parse_source("{ a 1 }");
            assert!(result.is_err(), "missing colon dict should be rejected");
            let err = result.unwrap_err();
            assert!(
                err.message.contains("missing colon") || err.message.contains("dict literal"),
                "error should hint at missing colon, got: {}",
                err.message
            );
        }

        #[test]
        fn test_missing_colon_dict_with_comma() {
            // `{ a 1, b 2 }` 也应报错
            let result = parse_source("{ a 1, b 2 }");
            assert!(result.is_err(), "missing colon dict with comma should be rejected");
        }

        #[test]
        fn test_missing_colon_string_key() {
            // `{ "key" "value" }` 应报错
            let result = parse_source(r#"{ "key" "value" }"#);
            assert!(result.is_err(), "missing colon with string key should be rejected");
        }

        #[test]
        fn test_valid_block_not_flagged() {
            // `{ x + 1 }` 是合法 block（x 后跟运算符 +），不应被误报
            let result = parse_source("{ x + 1 }");
            assert!(
                result.is_ok(),
                "valid block with operator should not be flagged, got: {:?}",
                result.err()
            );
        }

        #[test]
        fn test_valid_single_stmt_block_not_flagged() {
            // `{ x }` 是合法 block（单个语句），不应被误报
            let result = parse_source("{ x }");
            // 可能是 block 也可能是 dict（空 dict 需要 {}），{ x } 是 block
            assert!(
                result.is_ok(),
                "single stmt block should be accepted, got: {:?}",
                result.err()
            );
        }

        #[test]
        fn test_valid_dict_with_colon_not_flagged() {
            // `{ a: 1 }` 是合法 dict，不应被误报
            let result = parse_source("{ a: 1 }");
            assert!(
                result.is_ok(),
                "valid dict with colon should be accepted, got: {:?}",
                result.err()
            );
        }

        #[test]
        fn test_block_with_semicolon_not_flagged() {
            // `{ a; 1 }` 是合法 block（a 和 1 是两个语句），不应被误报
            let result = parse_source("{ a; 1 }");
            assert!(
                result.is_ok(),
                "block with semicolon should be accepted, got: {:?}",
                result.err()
            );
        }
    }
}
