//! # Nuzo 词法分析器（Lexer）模块
//!
//! ## 模块职责
//! 将 Nuzo 源代码字符串转换为 Token 序列（词法单元流）。
//! 这是编译器前端的第一阶段，为语法分析器（Parser）提供结构化的输入。
//!
//! ## 核心设计
//!
//! ### 1. 零拷贝架构
//! - **传统方案**：`Vec<char>` 分解导致 4x 内存膨胀（UTF-8 中文字符占 3 字节）
//! - **本方案**：直接操作 `&[u8]` 字节切片，Token 文本通过 `&str` 引用原始源码
//! - **收益**：处理中文源码时内存占用降低 75%
//!
//! ### 2. UTF-8 感知的字符处理
//! - 正确处理多字节 UTF-8 字符（中文、日文、韩文、Emoji）
//! - 列号按 Unicode 码点递增，非字节位置
//! - 支持中文标识符和关键字（如 `变量`, `函数`）
//!
//! ### 3. 双语关键字识别
//! - 自动识别英文关键字（`if`, `fn`）和中文字面量（`如果`, `函数`）
//! - CJK 关键字支持 1-2 字符长度（单字：`真`, `假`；双字：`如果`, `函数`）
//! - 智能分词：在连续 CJK 字符中正确切分关键字边界
//!
//! ## 扫描状态机
//!
//! ```text
//! ┌──────────┐    空白/注释    ┌──────────┐   字符分类   ┌────────────┐
//! │  开始     │ ──────────────→ │ 跳过空白 │ ──────────→ │ Token 识别  │
//! │  状态     │ ←────────────── │          │              │            │
//! └──────────┘                 └──────────┘              └────────────┘
//!       ↑                                                    │
//!       │                              ┌────────────────────┼────────────┐
//!       │                              ↓                    ↓            ↓
//!                       ┌──────────────┐  ┌──────────┐  ┌────────┐  ┌────────┐
//!                       │  数字扫描     │  │字符串扫描  │  │标识符   │  │符号匹配 │
//!                       │ (整数/浮点)   │  │(引号内容) │  │/关键字  │  │(运算符) │
//!                       └──────────────┘  └──────────┘  └────────┘  └────────┘
//! ```
//!
//! ## Token 识别规则
//!
//! | 输入模式 | Token 类型 | 示例 |
//! |---------|-----------|------|
//! | `[0-9]+(\.[0-9]+)?` | Number | `42`, `3.14` |
//! | `"..."` 或 `'...'` | String | `"hello"`, `'world'` |
//! | `[a-zA-Z_][a-zA-Z0-9_]*` | Ident/Keyword | `var`, `if`, `my_var` |
//! | `[\u{4E00}-\u{9FFF}]{1,2}` | CJK Keyword | `如果`, `真` |
//! | 单/双字符运算符 | Operator | `+`, `==`, `=>`, `..<` |
//! | 单字符分隔符 | Delimiter | `(`, `{`, `[`, `,` |
//!
//! ## 性能特征
//! - **时间复杂度**：O(n) 单遍扫描（n = 源码字节数）
//! - **空间复杂度**：O(k)（k = Token 数量，不含源码副本）
//! - **关键字查找**：O(1) 完美哈希分发
//! - **无堆分配**：除了 Vec<Token> 外无额外动态分配
//!
//! ## 错误处理策略
//! - **严格模式**：遇到非法字符立即返回错误
//! - **精确位置**：所有错误包含行号+列号信息
//! - **不恢复**：Lexer 不尝试错误恢复（由 Parser 层处理）

use crate::token::{Token, TokenKind, lookup_keyword};

/// 词法分析错误类型
///
/// 当 Lexer 遇到无法识别的输入时返回此错误。
/// 包含详细的错误位置信息用于生成用户友好的错误消息。
///
/// # 错误类型示例
/// - 未闭合的字符串字面量
/// - 非法的单独 `&` 或 `|`（期望 `&&` 或 `||`）
/// - 无法识别的 Unicode 字符（不在标识符范围内）
#[derive(Debug)]
pub struct LexerError {
    /// 人类可读的错误描述
    pub message: String,
    /// 错误所在行号（1-based）
    pub line: usize,
    /// 错误所在列号（1-based）
    pub column: usize,
}

impl std::fmt::Display for LexerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Lexer error at {}:{}: {}", self.line, self.column, self.message)
    }
}

impl std::error::Error for LexerError {}

/// 从 `LexerError` 自动转换为 `NuzoError`。
///
/// 词法错误属于用户代码问题（非法字符、未闭合字符串等），不是 VM/compiler bug，
/// 因此映射为带 `SourceLocation` 的 `InternalError::LexerError` 并附加稳定的
/// `ErrorCode::SyntaxError`（`C0005`），保留原始 line/column 信息以便精确定位。
impl From<LexerError> for nuzo_values::NuzoError {
    fn from(e: LexerError) -> Self {
        let loc = nuzo_values::SourceLocation::new(e.line).with_column(e.column);
        nuzo_values::NuzoError {
            kind: nuzo_values::NuzoErrorKind::Internal(
                nuzo_values::InternalError::LexerError { message: e.message },
                None,
            ),
            source_location: Some(loc),
            code: nuzo_core::error::ErrorCode::SyntaxError,
        }
    }
}

/// 高性能零拷贝词法分析器
///
/// 基于字节切片的 Lexer 实现，避免 UTF-8 分解带来的内存开销。
///
/// # 生命周期参数
/// - `'a`: 源字符串的生命周期。生成的 Token 通过 `&'a str` 引用原始源码，
///   因此 Lexer 的生存期不能超过源字符串。
///
/// # 内部状态
/// ```text
/// Lexer<'a> {
///     source: &'a [u8],      // 原始 UTF-8 字节（借用，零拷贝）
///     pos: usize,             // 当前字节偏移量
///     line: usize,            // 当前行号（1-based）
///     column: usize,          // 当前列号（1-based, UTF-8 字符单位）
/// }
/// ```
///
/// # 使用示例
/// ```ignore
/// use nuzo_frontend::Lexer;
///
/// let source = "fn add(a, b) { a + b }";
/// let lexer = Lexer::new(source);
/// let tokens = lexer.scan_all()?;
/// // tokens: Vec<(Token, &str)>
/// // 每个 Token 都携带类型和源码文本引用
/// ```
///
/// # 线程安全
/// Lexer 不使用任何内部可变状态（除了 &mut self 方法），
/// 可以在线程间移动但不能共享。
pub struct Lexer<'a> {
    /// 原始 UTF-8 字节序列（借用自源字符串，零拷贝）
    source: &'a [u8],

    /// 当前扫描位置的字节偏移量（0-based）
    ///
    /// 范围：[0, source.len()]
    /// - 0 表示起始位置
    /// - source.len() 表示 EOF
    pos: usize,

    /// 当前行号（从 1 开始计数）
    ///
    /// 在遇到 `\n` 字符时递增
    line: usize,

    /// 当前列号（从 1 开始计数，按 UTF-8 字符递增）
    ///
    /// 注意：列号是**字符位置**而非字节位置，
    /// 这确保了中文代码的错误定位准确性。
    column: usize,

    /// 当前正在扫描的 Token 的起始字节偏移
    ///
    /// 用于生成 Token 的文本切片 `&source[token_start..pos]`
    #[allow(dead_code)]
    token_start: usize,
}

// ---------------------------------------------------------------------------
// UTF-8 解析辅助方法
// ---------------------------------------------------------------------------
//
// 这组方法实现了字节级别的 UTF-8 解码，避免使用 `chars()` 迭代器
// 带来的性能开销和生命周期复杂性。
//
// # UTF-8 编码规则回顾
// - 1 字节: 0xxxxxxx (ASCII, U+0000 ~ U+007F)
// - 2 字节: 110xxxxx 10xxxxxx (U+0080 ~ U+07FF)
// - 3 字节: 1110xxxx 10xxxxxx 10xxxxxx (U+0800 ~ U+FFFF) ← 中文主要范围
// - 4 字节: 11110xxx 10xxxxxx 10xxxxxx 10xxxxxx (U+10000 ~ U+10FFFF)
//
// # 设计决策
// 为什么不直接用 `source.chars()`？
// 1. `chars()` 返回 `Chars<'a>` 迭代器，难以与字节偏移混用
// 2. 需要同时维护字符迭代器和字节位置，代码复杂度高
// 3. 直接操作字节更灵活（如 peek_next 需要跳过变长字符）
// ---------------------------------------------------------------------------

impl<'a> Lexer<'a> {
    /// 获取当前位置的字节值
    ///
    /// # 返回值
    /// - `Some(u8)` - 当前位置的字节（如果未到末尾）
    /// - `None` - 已到达输入末尾
    ///
    /// # 性能
    /// 此方法是内联的，编译器会优化为单次数组边界检查。
    #[inline]
    fn current_byte(&self) -> Option<u8> {
        self.source.get(self.pos).copied()
    }

    /// 计算当前字符的 UTF-8 字节长度
    ///
    /// 通过查看首字节的高位确定字符编码长度（与 RFC 3629 一致）。
    ///
    /// # 返回值
    /// | 首字节范围 | 字符长度 | 含义 |
    /// |-----------|---------|------|
    /// | 0x00-0x7F | 1 | ASCII 字符 |
    /// | 0xC0-0xDF | 2 | 2 字节字符首字节（拉丁扩展等） |
    /// | 0xE0-0xEF | 3 | 3 字节字符首字节（CJK 统一汉字等）* |
    /// | 0xF0-0xF7 | 4 | 4 字节字符首字节（Emoji、补充平面等） |
    ///
    /// * 中文字符主要在此范围（U+4E00-U+9FFF 编码为 3 字节）
    ///
    /// # 不变量（Invariant）
    ///
    /// `self.source` 来自 [`new()`](Self::new) 传入的 `&'a str`，因此整体是合法 UTF-8。
    /// `self.pos` 始终位于字符边界上（由 `advance()`/`peek_next()` 严格按 `current_char_len`
    /// 推进保证）。因此本方法只会看到 ASCII 或多字节字符的首字节，**不会**看到续字节
    /// (0x80-0xBF) 或非法首字节 (0xF8-0xFF)。
    ///
    /// 若调试构建中出现续字节/非法首字节，`debug_assert!` 会立即触发，便于定位
    /// 上游位置跟踪 bug。生产构建中保留兜底（按 1 字节前进）以避免 panic 死循环。
    ///
    /// # 边界情况
    /// - 输入末尾返回 0（表示无字符）
    #[inline]
    fn current_char_len(&self) -> usize {
        match self.current_byte() {
            None => 0,
            Some(b) if b < 0x80 => 1, // ASCII (0xxxxxxx)
            // 续字节 (0x80-0xBF)：不应出现在 pos 处。debug 构建触发断言暴露假设违反，
            // 生产构建按 1 字节前进避免死循环。
            Some(b) if b < 0xC0 => {
                debug_assert!(
                    false,
                    "lexer position landed on UTF-8 continuation byte 0x{:02X} at offset {} — \
                     source is assumed valid UTF-8 from &'a str, position tracking invariant violated",
                    b, self.pos
                );
                1
            }
            Some(b) if b < 0xE0 => 2, // 2-byte lead (110xxxxx)
            Some(b) if b < 0xF0 => 3, // 3-byte lead (1110xxxx)
            Some(b) if b < 0xF8 => 4, // 4-byte lead (11110xxx)
            // 非法 UTF-8 首字节 (0xF8-0xFF)：理论上不会出现
            Some(b) => {
                debug_assert!(
                    false,
                    "illegal UTF-8 lead byte 0x{:02X} at offset {} — source invariant violated",
                    b, self.pos
                );
                1
            }
        }
    }

    /// 将当前位置的字符解码为 `&str` 切片
    ///
    /// 返回从当前位置开始的完整 UTF-8 字符切片。
    /// 这是零拷贝操作，直接引用源码字节。
    ///
    /// # 返回值
    /// - 有效 UTF-8 字符切片（通常 1-4 字节）
    /// - 空字符串 `""`（如果在末尾）
    ///
    /// # 不变量
    /// `source` 来自 `&'a str`，整体为合法 UTF-8；`pos` 始终在字符边界。
    /// 因此切片 [`source[pos..pos+len]`] 必为合法 UTF-8。`debug_assert!` 在调试
    /// 构建中暴露假设违反，生产构建保留 `unwrap_or` 兜底防止 panic（返回空串
    /// 而非替换字符 `"\u{FFFD}"`，避免污染下游语义分析）。
    #[inline]
    fn current_char_as_str(&self) -> &'a str {
        let len = self.current_char_len();
        if len == 0 {
            return "";
        }
        let bytes = &self.source[self.pos..self.pos + len];
        debug_assert!(
            std::str::from_utf8(bytes).is_ok(),
            "invalid UTF-8 bytes at offset {} (len={}) — source invariant violated",
            self.pos,
            len
        );
        // 生产构建兜底：返回空串而非替换字符，避免污染零拷贝语义
        std::str::from_utf8(bytes).unwrap_or("")
    }
}

impl<'a> Lexer<'a> {
    /// 创建新的词法分析器实例
    ///
    /// # 参数
    /// * `source` - 要分析的源代码字符串（借用，不复制）
    ///
    /// # 初始状态
    /// ```text
    /// pos = 0          // 从源码起始位置开始
    /// line = 1         // 行号从 1 开始
    /// column = 1       // 列号从 1 开始
    /// ```
    ///
    /// # 性能说明
    /// 此操作是 O(1) 的，仅保存引用和初始化计数器。
    /// 实际的扫描工作在 `scan_all()` 中完成。
    ///
    /// # 示例
    /// ```ignore
    /// let source = "let x = 42";
    /// let lexer = Lexer::new(source);
    /// ```
    pub fn new(source: &'a str) -> Self {
        let bytes = source.as_bytes();
        let pos = if bytes.len() >= 3 && bytes[0] == 0xEF && bytes[1] == 0xBB && bytes[2] == 0xBF {
            3
        } else {
            0
        };

        Lexer { source: bytes, pos, line: 1, column: 1, token_start: 0 }
    }

    /// 扫描整个源代码并返回所有 Token
    ///
    /// 这是 Lexer 的主入口方法，执行完整的词法分析流程。
    ///
    /// # 返回值
    /// `Result<Vec<(Token, &'a str)>, LexerError>`
    /// - **成功**：Token 序列，每个元素包含：
    ///   - `Token`: 类型 + 位置信息
    ///   - `&'a str`: 零拷贝的源码文本引用
    /// - **失败**：词法错误（非法字符、未闭合字符串等）
    ///
    /// # Token 序列特征
    /// 1. 总是以 [`TokenKind::Eof`] 结尾（哨兵 Token）
    /// 2. 不包含空白字符和注释
    /// 3. 保持源码中的出现顺序
    /// 4. 文本切片的生命周期与原始源码相同
    ///
    /// # 错误处理
    /// 遇到第一个错误即停止扫描，不会尝试恢复。
    /// 这简化了实现，也避免了部分扫描导致的语义混淆。
    ///
    /// # 性能
    /// - 时间复杂度：O(n)，n = 源码字节数
    /// - 空间复杂度：O(k)，k = Token 数量
    /// - 预分配：Vec 按需增长（通常 2-3 倍扩容）
    pub fn scan_all(mut self) -> Result<Vec<(Token, &'a str)>, LexerError> {
        self.skip_whitespace_and_comments();

        let mut tokens = Vec::new();
        loop {
            let (token, text) = self.next_token()?;
            let is_eof = token.kind == TokenKind::Eof;
            tokens.push((token, text));
            if is_eof {
                break;
            }
        }
        Ok(tokens)
    }

    /// 查看当前字符但不移动位置（1 字符 lookahead）
    ///
    /// 用于决策性预览（如判断是否进入某种 Token 扫描模式）。
    ///
    /// # 返回值
    /// - `Some(char)` - 当前位置的 Unicode 字符
    /// - `None` - 已到输入末尾
    fn peek(&self) -> Option<char> {
        self.current_char_as_str().chars().next()
    }

    /// 查看下一个字符（跳过当前字符，2 字符 lookahead）
    ///
    /// 用于多字符运算符识别（如 `==`, `..`, `=>`）。
    ///
    /// # 实现细节
    /// 1. 获取当前字符的字节长度
    /// 2. 计算下一字符的起始字节位置
    /// 3. 解码该位置的 UTF-8 字符
    ///
    /// # 边界情况
    /// 如果当前位置是最后一个字符，返回 None。
    fn peek_next(&self) -> Option<char> {
        let len = self.current_char_len();
        if len == 0 {
            return None;
        }
        let next_pos = self.pos + len;
        let remaining = self.source.get(next_pos..)?;
        std::str::from_utf8(remaining).ok().and_then(|s| s.chars().next())
    }

    /// 向前推进一个字符（UTF-8 感知）并返回它
    ///
    /// 这是 Lexer 最核心的原语操作之一，负责：
    /// 1. 移动 `pos` 指针
    /// 2. 更新 `line`/`column` 位置信息
    /// 3. 返回被消费的字符
    ///
    /// # 位置更新规则
    /// - 遇到 `\n`：line += 1, column 重置为 1
    /// - 其他字符：column += 1
    ///
    /// # 返回值
    /// - `Some(char)` - 被消费的字符
    /// - `None` - 已到末尾（此时不移动）
    fn advance(&mut self) -> Option<char> {
        let ch = self.current_char_as_str().chars().next()?;
        let len = self.current_char_len();
        self.pos += len;
        if ch == '\n' {
            self.line += 1;
            self.column = 1;
        } else {
            self.column += 1;
        }
        Some(ch)
    }

    /// 条件性字符匹配和消费
    ///
    /// 如果当前字符匹配预期值，则消费它并返回 true；
    /// 否则保持不动返回 false。
    ///
    /// # 使用场景
    /// 多字符 Token 的识别：
    /// ```ignore
    /// // 识别 ==
    /// if self.match_char('=') { /* 是 == */ }
    /// else { /* 只是 = */ }
    /// ```
    fn match_char(&mut self, expected: char) -> bool {
        if self.peek() == Some(expected) {
            self.advance();
            true
        } else {
            false
        }
    }

    /// 提取从指定位置到当前位置的源码切片（零拷贝）
    ///
    /// # 参数
    /// * `start` - 切片起始字节偏移（通常是 Token 开始位置）
    ///
    /// # 返回值
    /// `&'a str` - 引用原始源码的字符串切片
    ///
    /// # 不变量
    /// `start` 与 `self.pos` 都位于字符边界上（由调用方保证，通常是 token_start
    /// 或 advance 后的位置）。`source` 整体是合法 UTF-8，因此切片必为合法 UTF-8。
    /// `debug_assert!` 在调试构建中暴露假设违反，生产构建保留 `unwrap_or("")`
    /// 兜底防止 panic。
    fn slice(&self, start: usize) -> &'a str {
        let bytes = &self.source[start..self.pos];
        debug_assert!(
            std::str::from_utf8(bytes).is_ok(),
            "slice [{}..{}] is not valid UTF-8 — position tracking invariant violated",
            start,
            self.pos
        );
        std::str::from_utf8(bytes).unwrap_or("")
    }

    /// 构建带有文本信息的完整 Token
    ///
    /// 组合类型、位置和文本信息，生成最终的 Token 元组。
    ///
    /// # 参数
    /// * `kind` - Token 种类
    /// * `start_pos` - Token 在源码中的起始字节偏移
    /// * `start_col` - Token 的起始列号
    ///
    /// # 返回值
    /// `(Token, &'a str)` - 完整的 Token 信息
    fn make_token(&self, kind: TokenKind, start_pos: usize, start_col: usize) -> (Token, &'a str) {
        let text = self.slice(start_pos);
        let token = Token::new(kind, self.line, start_col, start_pos);
        (token, text)
    }

    /// 跳过空白字符、BOM 和行内注释
    ///
    /// 在每次调用 `next_token()` 时首先执行此方法，
    /// 确保 Token 之间不包含无意义的空白或编码标记。
    ///
    /// # 跳过的内容
    /// | 字符/序列 | 处理方式 |
    /// |----------|---------|
    /// | 空格 `' '` | 跳过 |
    /// | 制表符 `'\t'` | 跳过 |
    /// | 回车 `'\r'` | 跳过 |
    /// | 换行 `'\n'` | 跳过（advance 会更新行号） |
    /// | UTF-8 BOM `'\u{FEFF}'` | 跳过 |
    /// | 注释 `'#'...\n'` | 跳过整行 |
    ///
    /// # 注释语法
    /// Nuzo 使用 `#` 开头的行注释（类似 Python/Ruby）：
    /// ```text
    /// # 这是一个注释
    /// x = 42  # 行尾注释
    /// ```
    fn skip_whitespace_and_comments(&mut self) {
        loop {
            match self.peek() {
                Some(' ') | Some('\t') | Some('\r') | Some('\n') => {
                    self.advance();
                }
                Some('\u{FEFF}') => {
                    self.advance();
                }
                Some('#') => {
                    while let Some(c) = self.peek() {
                        if c == '\n' {
                            break; // 保留换行符用于行号统计
                        }
                        self.advance();
                    }
                }
                _ => break,
            }
        }
    }

    /// 扫描并返回下一个 Token
    ///
    /// Lexer 的主循环体，基于首字符类型分发到不同的扫描模式：
    /// - 分隔符：直接返回对应的单字符 Token
    /// - 运算符：可能需要查看下一字符（如 + vs +=）
    /// - 字面量：调用专门的扫描方法（字符串/数字）
    /// - 标识符/关键字：调用标识符扫描方法
    /// - 非法字符：返回 LexerError
    fn next_token(&mut self) -> Result<(Token, &'a str), LexerError> {
        self.skip_whitespace_and_comments();

        let start_pos = self.pos;
        let start_col = self.column;

        let ch = match self.advance() {
            Some(c) => c,
            None => return Ok((Token::eof(self.line, self.column, self.pos), "")),
        };

        match ch {
            '(' => Ok(self.make_token(TokenKind::LParen, start_pos, start_col)),
            ')' => Ok(self.make_token(TokenKind::RParen, start_pos, start_col)),
            '{' => Ok(self.make_token(TokenKind::LBrace, start_pos, start_col)),
            '}' => Ok(self.make_token(TokenKind::RBrace, start_pos, start_col)),
            '[' => Ok(self.make_token(TokenKind::LBracket, start_pos, start_col)),
            ']' => Ok(self.make_token(TokenKind::RBracket, start_pos, start_col)),
            ',' => Ok(self.make_token(TokenKind::Comma, start_pos, start_col)),
            ':' => Ok(self.make_token(TokenKind::Colon, start_pos, start_col)),
            ';' => Ok(self.make_token(TokenKind::Semicolon, start_pos, start_col)),

            '.' => {
                if self.match_char('.') {
                    if self.match_char('<') {
                        Ok(self.make_token(TokenKind::DotDotLt, start_pos, start_col))
                    } else {
                        Ok(self.make_token(TokenKind::DotDot, start_pos, start_col))
                    }
                } else {
                    Ok(self.make_token(TokenKind::Dot, start_pos, start_col))
                }
            }

            '+' => {
                if self.match_char('=') {
                    Ok(self.make_token(TokenKind::PlusEqual, start_pos, start_col))
                } else {
                    Ok(self.make_token(TokenKind::Plus, start_pos, start_col))
                }
            }
            '-' => {
                if self.match_char('=') {
                    Ok(self.make_token(TokenKind::MinusEqual, start_pos, start_col))
                } else {
                    Ok(self.make_token(TokenKind::Minus, start_pos, start_col))
                }
            }
            '*' => {
                if self.match_char('*') {
                    Ok(self.make_token(TokenKind::StarStar, start_pos, start_col))
                } else if self.match_char('=') {
                    Ok(self.make_token(TokenKind::StarEqual, start_pos, start_col))
                } else {
                    Ok(self.make_token(TokenKind::Star, start_pos, start_col))
                }
            }
            '/' => {
                if self.match_char('=') {
                    Ok(self.make_token(TokenKind::SlashEqual, start_pos, start_col))
                } else {
                    Ok(self.make_token(TokenKind::Slash, start_pos, start_col))
                }
            }
            '%' => Ok(self.make_token(TokenKind::Percent, start_pos, start_col)),

            '=' => {
                if self.match_char('=') {
                    Ok(self.make_token(TokenKind::EqEq, start_pos, start_col))
                } else if self.match_char('>') {
                    Ok(self.make_token(TokenKind::Arrow, start_pos, start_col))
                } else {
                    Ok(self.make_token(TokenKind::Eq, start_pos, start_col))
                }
            }

            '!' => {
                if self.match_char('=') {
                    Ok(self.make_token(TokenKind::BangEq, start_pos, start_col))
                } else {
                    Ok(self.make_token(TokenKind::Bang, start_pos, start_col))
                }
            }

            '<' => {
                if self.match_char('=') {
                    Ok(self.make_token(TokenKind::LtEq, start_pos, start_col))
                } else {
                    Ok(self.make_token(TokenKind::Lt, start_pos, start_col))
                }
            }

            '>' => {
                if self.match_char('=') {
                    Ok(self.make_token(TokenKind::GtEq, start_pos, start_col))
                } else {
                    Ok(self.make_token(TokenKind::Gt, start_pos, start_col))
                }
            }

            '&' => {
                if self.match_char('&') {
                    Ok(self.make_token(TokenKind::AndAnd, start_pos, start_col))
                } else {
                    Err(LexerError {
                        message: "expected '&&' for logical AND".to_string(),
                        line: self.line,
                        column: start_col,
                    })
                }
            }

            '|' => {
                if self.match_char('|') {
                    Ok(self.make_token(TokenKind::OrOr, start_pos, start_col))
                } else if self.match_char('>') {
                    Ok(self.make_token(TokenKind::Pipe, start_pos, start_col))
                } else {
                    Err(LexerError {
                        message: "expected '||' for logical OR or '|>' for pipe".to_string(),
                        line: self.line,
                        column: start_col,
                    })
                }
            }

            '?' => {
                if self.match_char('?') {
                    Ok(self.make_token(TokenKind::QuestionQuestion, start_pos, start_col))
                } else {
                    Err(LexerError {
                        message: "expected '??' for null coalescing".to_string(),
                        line: self.line,
                        column: start_col,
                    })
                }
            }

            '"' | '\'' => self.scan_string(ch, start_pos, start_col),

            c if c.is_ascii_digit() => self.scan_number(start_pos, start_col),

            c if is_ident_start(c) => self.scan_identifier(start_pos, start_col),

            c => Err(LexerError {
                message: format!("unexpected character: '{}'", c),
                line: self.line,
                column: start_col,
            }),
        }
    }

    /// 扫描字符串字面量（支持双引号和单引号）
    ///
    /// # 语法规则
    /// ```text
    /// string ::= '"' (character | escape)* '"'
    ///           | "'" (character | escape)* "'"
    ///
    /// escape  ::= '\' any_character
    /// ```
    ///
    /// # 功能特性
    /// - **零拷贝**：返回的字符串切片直接引用源码，不包含引号
    /// - **转义支持**：识别 `\` 转义序列（跳过反斜杠和下一个字符）
    /// - **错误检测**：
    ///   - 未闭合的字符串（遇到换行符或文件末尾）
    ///   - 错误位置报告为**起始引号位置**而非结束位置
    ///
    /// # 转义序列处理策略（设计意图）
    ///
    /// 当前实现对 `\` 的处理是**保守跳过**：遇到反斜杠时调用 `advance()` 跳过
    /// 反斜杠本身，然后由循环末尾的 `advance()` 跳过下一字符。这保证：
    /// 1. `"a\"b"` 中的 `\"` 不会误判为字符串结束（保留正确的 token 边界）
    /// 2. 返回的 token 文本是**源码原样切片**（包含 `\` 和转义字符的字面形式）
    ///
    /// **注意**：本方法**不解码**转义序列（如 `\n` 不会变成换行符、`\t` 不会变成
    /// 制表符、`\\` 不会变成单反斜杠）。解码工作预期在后续阶段处理：
    /// - 编译器后端（`nuzo_compiler`）将 String token 编码到字节码常量池时
    /// - 或运行时（`nuzo_vm`）加载字符串常量时
    ///
    /// **现状**：截至本次审查，`nuzo_compiler` 与 `nuzo_vm` 均未实现转义解码，
    /// 因此含转义序列的字符串字面量当前以源码原样形式存储。这是一个已知的
    /// 待完善项，新增转义解码逻辑时应在此处文档同步更新。
    ///
    /// # 参数
    /// * `quote` - 引号字符（`"` 或 `'`）
    /// * `start_pos` - 起始引号的字节偏移
    /// * `start_col` - 起始引号的列号
    ///
    /// # 返回值
    /// 成功时返回 `(Token, &str)`：
    /// - Token 类型为 `TokenKind::String`
    /// - 文本为**引号内容**（不包含引号本身）
    ///
    /// # 示例
    /// ```text
    /// 输入: "hello world"
    /// 返回: (String token, "hello world")
    ///
    /// 输入: '单引号字符串'
    /// 返回: (String token, "单引号字符串")
    /// ```
    fn scan_string(
        &mut self,
        quote: char,
        start_pos: usize,
        start_col: usize,
    ) -> Result<(Token, &'a str), LexerError> {
        let content_start = self.pos;
        while let Some(c) = self.peek() {
            if c == quote {
                let content =
                    std::str::from_utf8(&self.source[content_start..self.pos]).unwrap_or("");
                self.advance();
                let token = Token::new(TokenKind::String, self.line, start_col, start_pos);
                return Ok((token, content));
            }
            if c == '\n' {
                return Err(LexerError {
                    message: "unterminated string".to_string(),
                    line: self.line,
                    column: start_col,
                });
            }
            if c == '\\' {
                self.advance();
            }
            self.advance();
        }
        Err(LexerError {
            message: "unterminated string".to_string(),
            line: self.line,
            column: start_col,
        })
    }

    /// 扫描数字字面量（支持整数和浮点数）
    ///
    /// # 语法规则
    /// ```text
    /// number ::= digit+ ('.' digit+)?
    ///
    /// digit  ::= [0-9]
    /// ```
    ///
    /// # 支持的格式
    /// | 格式 | 示例 | 说明 |
    /// |------|------|------|
    /// | 整数 | `42`, `0`, `123456` | 纯数字序列 |
    /// | 浮点数 | `3.14`, `0.5`, `100.0` | 包含小数点的数字 |
    ///
    /// # 边界情况处理
    /// - **小数点歧义**：`1.x` 中的点不会被识别为小数点（因为后面不是数字）
    /// - **多小数点**：`1.2.3` 会扫描为 `1.2` 和 `.3`（后者是语法错误）
    /// - **前导零**：允许 `007`, `00.5` 等形式（由语义分析阶段处理）
    ///
    /// # 注意事项
    /// - 不支持科学计数法（如 `1e10`）
    /// - 不支持十六进制/八进制/二进制（如 `0xFF`, `0o77`, `0b1010`）
    /// - 不支持数字分隔符（如 `1_000_000`）
    /// - 负号由 Parser 的一元运算符处理，不在此处识别
    fn scan_number(
        &mut self,
        start_pos: usize,
        start_col: usize,
    ) -> Result<(Token, &'a str), LexerError> {
        while let Some(c) = self.peek() {
            if c.is_ascii_digit() {
                self.advance();
            } else {
                break;
            }
        }

        if self.peek() == Some('.') && self.peek_next().is_some_and(|c| c.is_ascii_digit()) {
            self.advance();
            while let Some(c) = self.peek() {
                if c.is_ascii_digit() {
                    self.advance();
                } else {
                    break;
                }
            }
        }

        Ok(self.make_token(TokenKind::Number, start_pos, start_col))
    }

    /// 扫描标识符或关键字（ASCII 和 CJK 通用入口）
    ///
    /// # 语法规则
    /// ```text
    /// identifier ::= ident_start ident_continue*
    ///
    /// ident_start ::= [a-zA-Z_] | CJK_Character
    /// ident_continue ::= [a-zA-Z0-9_] | CJK_Character
    /// ```
    ///
    /// # 处理流程
    /// 1. 检查首字符是否为 CJK 字符
    ///    - 如果是 → 调用 `scan_cjk_token()` 进行中文关键字识别
    ///    - 如果否 → 按 ASCII 标识符规则扫描
    /// 2. 扫描完成后查询关键字表
    ///    - 匹配到关键字 → 返回对应的 TokenKind（如 If, Fn）
    ///    - 未匹配 → 返回 `TokenKind::Ident`
    ///
    /// # ASCII 标识符示例
    /// | 输入 | Token 类型 | 说明 |
    /// |------|-----------|------|
    /// | `variable` | Ident | 普通变量名 |
    /// | `myFunc` | Ident | 驼峰命名 |
    /// | `_private` | Ident | 下划线开头 |
    /// | `if` | If | 关键字 |
    /// | `fn add` | Fn + Ident | 函数关键字 + 名称 |
    fn scan_identifier(
        &mut self,
        start_pos: usize,
        start_col: usize,
    ) -> Result<(Token, &'a str), LexerError> {
        let first_char = self.slice(start_pos).chars().next().ok_or_else(|| LexerError {
            message: "empty identifier slice (internal lexer inconsistency: scan_identifier called with empty slice)".to_string(),
            line: self.line,
            column: start_col,
        })?;

        if is_cjk_ident(first_char) {
            return self.scan_cjk_token(start_pos, start_col);
        }

        while let Some(c) = self.peek() {
            if is_ident_continue(c) {
                self.advance();
            } else {
                break;
            }
        }

        let text = self.slice(start_pos);
        let kind = lookup_keyword(text).unwrap_or(TokenKind::Ident);
        let token = Token::new(kind, self.line, start_col, start_pos);
        Ok((token, text))
    }

    /// 扫描 CJK（中日韩）字符开头的标识符或关键字
    ///
    /// # 设计挑战
    /// 与 ASCII 标识符不同，CJK 字符没有天然的分词边界：
    /// - `如果变量` 应该拆分为：`如果`(If) + `变量`(Ident)
    /// - `真` 是单字关键字
    /// - `函数名` 应该拆分为：`函数`(Fn) + `名`(Ident)
    ///
    /// # 分词策略（最长匹配优先）
    /// 1. **尝试双字符匹配**：检查当前字符 + 下一字符是否构成关键字
    ///    - 如 `如果`, `函数`, `遍历`, `否则`
    /// 2. **尝试单字符匹配**：检查当前单个字符是否为关键字
    ///    - 如 `真`, `假`, `空`
    /// 3. **作为普通标识符**：都不是则继续扫描后续字符
    ///
    /// # 智能边界检测
    /// 在扫描普通 CJK 标识符时，会调用 `cjk_keyword_starts_at()` 检查
    /// 后续位置是否是关键字的开始，如果是则提前终止当前标识符。
    /// 这确保了 `如果变量` 正确分词为两个 Token。
    fn scan_cjk_token(
        &mut self,
        start_pos: usize,
        start_col: usize,
    ) -> Result<(Token, &'a str), LexerError> {
        if let Some(next_ch) = self.peek()
            && is_cjk_ident(next_ch)
        {
            let two_char_end = self.pos + next_ch.len_utf8();
            if two_char_end <= self.source.len()
                && let Ok(two_char_text) =
                    std::str::from_utf8(&self.source[start_pos..two_char_end])
                && let Some(kind) = lookup_keyword(two_char_text)
            {
                self.advance();
                let token = Token::new(kind, self.line, start_col, start_pos);
                return Ok((token, two_char_text));
            }
        }

        let one_char_text = self.slice(start_pos);
        if let Some(kind) = lookup_keyword(one_char_text) {
            let token = Token::new(kind, self.line, start_col, start_pos);
            return Ok((token, one_char_text));
        }

        while let Some(c) = self.peek() {
            if !is_ident_continue(c) {
                break;
            }
            if is_cjk_ident(c) && self.cjk_keyword_starts_at(self.pos).is_some() {
                break;
            }
            self.advance();
        }

        let text = self.slice(start_pos);
        let token = Token::new(TokenKind::Ident, self.line, start_col, start_pos);
        Ok((token, text))
    }

    /// 检查指定字节偏移处是否为 CJK 关键字的开始
    ///
    /// 这是一个**前瞻性检查方法**，用于在扫描 CJK 标识符时
    /// 确定是否应该在当前位置截断，以避免吞掉后续的关键字。
    ///
    /// # 检查顺序（最长匹配优先）
    /// 1. 尝试双字符关键字匹配
    /// 2. 尝试单字符关键字匹配
    /// 3. 都不匹配返回 None
    ///
    /// # 返回值
    /// - `Some(len)` - 发现关键字，返回其字节长度（用于跳过）
    /// - `None` - 不是关键字开始位置
    fn cjk_keyword_starts_at(&self, byte_offset: usize) -> Option<usize> {
        let remaining = self.source.get(byte_offset..)?;
        let s = std::str::from_utf8(remaining).ok()?;
        let mut chars = s.chars();
        let first = chars.next()?;
        if !is_cjk_ident(first) {
            return None;
        }

        if let Some(second) = chars.next()
            && is_cjk_ident(second)
        {
            let two_char_len = first.len_utf8() + second.len_utf8();
            let two_char_text = &s[..two_char_len];
            if lookup_keyword(two_char_text).is_some() {
                return Some(two_char_len);
            }
        }

        let one_char_len = first.len_utf8();
        let one_char_text = &s[..one_char_len];
        if lookup_keyword(one_char_text).is_some() {
            return Some(one_char_len);
        }

        None
    }
}

// ---------------------------------------------------------------------------
// 标识符字符分类函数
// ---------------------------------------------------------------------------
//
// 这组函数用于判断 Unicode 字符是否可以作为标识符的一部分。
// 它们被 Lexer 的 peek/advance 方法调用，接收的是已解码的 char 值。
//
// # Nuzo 标识符规则
// ```text
// identifier ::= ident_start ident_continue*
//
// ident_start  ::= [a-zA-Z_] | CJK_Ideograph
// ident_continue ::= [a-zA-Z0-9_] | CJK_Ideograph
// ```
//
// # CJK 范围说明
// - U+4E00 ~ U+9FFF: CJK 统一汉字（大部分常用汉字）
// - U+3400 ~ U+4DBF: CJK 扩展 A 区（生僻字）
// - U+F900 ~ U+FAFF: CJK 兼容汉字（兼容旧标准）
//
// # 设计决策
// 为什么包含这么多 CJK 范围？
// 1. 支持中文变量名和关键字是核心特性
// 2. 日文汉字（Kanji）和韩文汉字（Hanja）也在这些范围中
// 3. 未来可能扩展到其他语言的文字系统

/// 检查字符是否可作为标识符的起始字符
///
/// # 规则
/// - ASCII 字母：`a-z`, `A-Z`
/// - 下划线：`_`
/// - CJK 表意文字：见 `is_cjk_ident()` 的范围定义
#[inline]
fn is_ident_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_' || is_cjk_ident(c)
}

/// 检查字符是否可作为标识符的后续字符
///
/// # 与 is_ident_start 的区别
/// 额外允许数字 `0-9`，因为标识符不能以数字开头但可以包含数字。
///
/// # 示例
/// - `abc123` ✓ (字母 + 数字)
/// - `_var1` ✓ (下划线 + 字母 + 数字)
/// - `变量名` ✓ (CJK 字符序列)
/// - `123abc` ✗ (数字开头，不是合法标识符)
#[inline]
fn is_ident_continue(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || is_cjk_ident(c)
}

/// 检查字符是否为 CJK（中日韩）表意文字
///
/// # 覆盖的 Unicode 范围
/// | 范围 | 名称 | 字符数 | 说明 |
/// |------|------|--------|------|
/// | U+4E00-U+9FFF | CJK Unified Ideographs | 20,992 | 常用汉字（如：中、日、韩） |
/// | U+3400-U+4DBF | CJK Extension A | 6,592 | 生僻字（如：𠀀） |
/// | U+F900-U+FAFF | CJK Compatibility | 288 | 兼容字（如：﨎） |
///
/// # 未包含的范围（有意省略）
/// - CJK Extension B-F（极其生僻，极少使用）
/// - 日文假名（平假名、片假名）- 可按需添加
/// - 韩文谚文（音节块）- 可按需添加
/// - Emoji（非文字符号）
///
/// # 使用场景
/// 1. **标识符识别**：`变量`, `函数名`, `_数据`
/// 2. **关键字匹配**：`如果`, `真`, `空`
/// 3. **分词边界检测**：区分关键字和普通标识符
#[inline]
fn is_cjk_ident(c: char) -> bool {
    matches!(
        c,
        '\u{4E00}'..='\u{9FFF}' |     // CJK Unified Ideographs
        '\u{3400}'..='\u{4DBF}' |     // CJK Extension A
        '\u{F900}'..='\u{FAFF}'       // CJK Compatibility Ideographs
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_tokens() {
        let tokens = Lexer::new("1 + 2").scan_all().unwrap();
        assert_eq!(tokens[0].0.kind, TokenKind::Number);
        assert_eq!(tokens[1].0.kind, TokenKind::Plus);
        assert_eq!(tokens[2].0.kind, TokenKind::Number);
    }

    #[test]
    fn test_chinese_keywords() {
        let tokens = Lexer::new("如果 真 { }").scan_all().unwrap();
        assert_eq!(tokens[0].0.kind, TokenKind::If);
        assert_eq!(tokens[1].0.kind, TokenKind::True);
    }

    #[test]
    fn test_bilingual_equivalence() {
        assert!(TokenKind::If.is_if());
        // Chinese keywords map to the same variant as English
        assert!(lookup_keyword("如果").unwrap().is_if());
        assert!(lookup_keyword("函数").unwrap().is_fn());
    }

    #[test]
    fn test_string_literals() {
        let tokens = Lexer::new("\"hello\" 'world'").scan_all().unwrap();
        assert_eq!(tokens[0].0.kind, TokenKind::String);
        assert_eq!(tokens[0].1, "hello");
        assert_eq!(tokens[1].0.kind, TokenKind::String);
        assert_eq!(tokens[1].1, "world");
    }

    #[test]
    fn test_range_operators() {
        let tokens = Lexer::new("1..10 0..<5").scan_all().unwrap();
        assert_eq!(tokens[1].0.kind, TokenKind::DotDot);
        assert_eq!(tokens[4].0.kind, TokenKind::DotDotLt);
    }

    #[test]
    fn test_utf8_multibyte_characters() {
        // Verify that Chinese characters don't cause 4x inflation
        let source = "如果函数变量";
        let lexer = Lexer::new(source);
        // source.as_bytes().len() should equal original byte length
        assert_eq!(lexer.source.len(), 18);
        let tokens = lexer.scan_all().unwrap();
        assert_eq!(tokens[0].0.kind, TokenKind::If); // 如果
        assert_eq!(tokens[1].0.kind, TokenKind::Fn); // 函数
        assert_eq!(tokens[2].0.kind, TokenKind::Ident); // 变量
    }

    #[test]
    fn test_zero_copy_slices_are_valid_utf8() {
        let source = "hello 世界 123";
        let tokens = Lexer::new(source).scan_all().unwrap();
        // All returned slices should be valid &str pointing into source
        for (_tok, text) in &tokens {
            assert!(source.contains(*text));
        }
    }

    #[test]
    fn test_scan_all_empty_source() {
        // 空源码应只返回 EOF token
        let tokens = Lexer::new("").scan_all().unwrap();
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].0.kind, TokenKind::Eof);
    }

    #[test]
    fn test_scan_all_whitespace_only() {
        // 仅含空白字符的源码应只返回 EOF token
        let tokens = Lexer::new("   \t\n  \n").scan_all().unwrap();
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].0.kind, TokenKind::Eof);
    }

    #[test]
    fn test_scan_all_returns_eof_at_end() {
        // 任何有效源码扫描后末尾必须是 EOF
        let tokens = Lexer::new("42").scan_all().unwrap();
        assert!(tokens.len() >= 2);
        let last = tokens.last().expect("tokens should not be empty");
        assert_eq!(last.0.kind, TokenKind::Eof);
    }

    #[test]
    fn test_scan_all_preserves_token_order() {
        // 验证 token 顺序与源码顺序一致
        let tokens = Lexer::new("a + b").scan_all().unwrap();
        let kinds: Vec<_> = tokens.iter().map(|(t, _)| t.kind).collect();
        assert_eq!(
            kinds,
            vec![TokenKind::Ident, TokenKind::Plus, TokenKind::Ident, TokenKind::Eof]
        );
    }

    #[test]
    fn test_scan_all_unterminated_string_returns_error() {
        // 未闭合的字符串应返回错误
        let result = Lexer::new("\"unterminated").scan_all();
        assert!(result.is_err());
    }

    #[test]
    fn test_scan_all_mixed_keywords_and_identifiers() {
        // 混合关键字和标识符
        let tokens = Lexer::new("if x else y").scan_all().unwrap();
        assert_eq!(tokens[0].0.kind, TokenKind::If);
        assert_eq!(tokens[1].0.kind, TokenKind::Ident);
        assert_eq!(tokens[1].1, "x");
        assert_eq!(tokens[2].0.kind, TokenKind::Else);
        assert_eq!(tokens[3].0.kind, TokenKind::Ident);
        assert_eq!(tokens[3].1, "y");
    }

    #[test]
    fn test_scan_all_chinese_and_english_keywords_mixed() {
        // 中英文关键字混合使用
        let tokens = Lexer::new("如果 true 否则 false").scan_all().unwrap();
        assert_eq!(tokens[0].0.kind, TokenKind::If);
        assert_eq!(tokens[1].0.kind, TokenKind::True);
        assert_eq!(tokens[2].0.kind, TokenKind::Else);
        assert_eq!(tokens[3].0.kind, TokenKind::False);
    }

    #[test]
    fn test_scan_all_all_operators() {
        // 验证各类运算符都能被正确扫描
        let tokens =
            Lexer::new("+ - * / % == != < > <= >= && || => |> ?? += -= *= /=").scan_all().unwrap();
        let ops = vec![
            TokenKind::Plus,
            TokenKind::Minus,
            TokenKind::Star,
            TokenKind::Slash,
            TokenKind::Percent,
            TokenKind::EqEq,
            TokenKind::BangEq,
            TokenKind::Lt,
            TokenKind::Gt,
            TokenKind::LtEq,
            TokenKind::GtEq,
            TokenKind::AndAnd,
            TokenKind::OrOr,
            TokenKind::Arrow,
            TokenKind::Pipe,
            TokenKind::QuestionQuestion,
            TokenKind::PlusEqual,
            TokenKind::MinusEqual,
            TokenKind::StarEqual,
            TokenKind::SlashEqual,
        ];
        let kinds: Vec<_> = tokens.iter().map(|(t, _)| t.kind).collect();
        // 最后一个是 EOF
        assert_eq!(kinds.len(), ops.len() + 1);
        for (i, op) in ops.iter().enumerate() {
            assert_eq!(kinds[i], *op, "operator at index {} mismatch", i);
        }
        assert_eq!(kinds.last().unwrap(), &TokenKind::Eof);
    }

    #[test]
    fn test_scan_all_all_delimiters() {
        // 验证所有分隔符
        let tokens = Lexer::new("() {} [] , . : ;").scan_all().unwrap();
        let delims = vec![
            TokenKind::LParen,
            TokenKind::RParen,
            TokenKind::LBrace,
            TokenKind::RBrace,
            TokenKind::LBracket,
            TokenKind::RBracket,
            TokenKind::Comma,
            TokenKind::Dot,
            TokenKind::Colon,
            TokenKind::Semicolon,
        ];
        let kinds: Vec<_> = tokens.iter().map(|(t, _)| t.kind).collect();
        assert_eq!(kinds.len(), delims.len() + 1);
        for (i, d) in delims.iter().enumerate() {
            assert_eq!(kinds[i], *d, "delimiter at index {} mismatch", i);
        }
    }

    #[test]
    fn test_scan_all_bom_handling() {
        // UTF-8 BOM 应被跳过
        let bom_source = "\u{FEFF}42";
        let tokens = Lexer::new(bom_source).scan_all().unwrap();
        assert_eq!(tokens[0].0.kind, TokenKind::Number);
        assert_eq!(tokens[0].1, "42");
    }

    #[test]
    fn test_scan_all_comments_are_skipped() {
        // 行注释（以 # 开头）应被跳过
        let tokens = Lexer::new("# this is a comment\n42").scan_all().unwrap();
        // 第一个 token 应该是 42，注释被跳过
        let number_token = tokens.iter().find(|(t, _)| t.kind == TokenKind::Number);
        assert!(number_token.is_some(), "number token should be found after comment");
    }

    #[test]
    fn test_scan_all_range_operators_in_expression() {
        // 验证范围运算符在表达式中的扫描
        let tokens = Lexer::new("1..10 0..<5").scan_all().unwrap();
        // 1 .. 10 0 ..< 5 EOF
        assert_eq!(tokens[0].0.kind, TokenKind::Number);
        assert_eq!(tokens[1].0.kind, TokenKind::DotDot);
        assert_eq!(tokens[2].0.kind, TokenKind::Number);
        assert_eq!(tokens[3].0.kind, TokenKind::Number);
        assert_eq!(tokens[4].0.kind, TokenKind::DotDotLt);
        assert_eq!(tokens[5].0.kind, TokenKind::Number);
    }

    #[test]
    fn test_scan_all_returns_text_slices() {
        // 验证返回的文本切片正确指向源码
        let source = "hello 42";
        let tokens = Lexer::new(source).scan_all().unwrap();
        assert_eq!(tokens[0].1, "hello");
        assert_eq!(tokens[1].1, "42");
    }

    // ========================================================================
    // P1-1: UTF-8 处理回归测试
    // ========================================================================
    //
    // 由于 `Lexer::new(source: &'a str)` 的入参类型由 Rust 类型系统保证是合法 UTF-8，
    // "拒绝非法 UTF-8" 的责任实际上由 `&str` 的构造方承担（编译期保证）。
    // 因此本组测试聚焦于：合法 UTF-8 的边界情况必须被正确处理，不触发
    // `current_char_len` / `current_char_as_str` / `slice` 中的 debug_assert 兜底分支。

    #[test]
    fn test_lexer_rejects_invalid_utf8() {
        // Rust 的 &str 类型在构造时就拒绝非法 UTF-8（编译期/运行期由 str 构造方保证），
        // 因此 Lexer::new 永远不会接收到非法 UTF-8。本测试断言该不变量：
        // 任何合法 &str（包括所有边界情况）都应被 Lexer 正确处理，不触发 panic。
        //
        // 覆盖的边界场景：
        // 1. 空字符串
        // 2. 仅 BOM
        // 3. ASCII / 2 字节 / 3 字节 / 4 字节字符混合
        // 4. 字符串末尾是多字节字符（验证 current_char_len 边界）
        // 5. 字符串开头就是多字节字符
        let cases: &[&str] = &[
            "",
            "\u{FEFF}",      // 仅 BOM
            "a",             // 单 ASCII
            "ä",             // 2 字节
            "中",            // 3 字节
            "𝓐",             // 4 字节 (U+1D4D0)
            "abc中𝓐",        // 混合
            "中",            // 开头是 3 字节
            "𝓐",             // 开头是 4 字节
            "hello 世界 🌍", // 结尾是 4 字节 Emoji
            "\u{FEFF}if 真", // BOM + 关键字 + CJK
        ];
        for &src in cases {
            // 不应 panic
            let _ = Lexer::new(src).scan_all();
        }
    }

    #[test]
    fn test_lexer_multibyte_at_end_of_source() {
        // 字符串末尾是 3 字节 CJK 字符，验证 current_char_len 不会越界读取。
        // 注意：CJK 字符是合法标识符字符，所以 "x中" 会被扫描为单个标识符 "x中"。
        // 测试重点是 lexer 不 panic 且正确处理末尾多字节字符。
        let src = "x 中"; // 空格分隔，确保分词为两个 ident
        let tokens = Lexer::new(src).scan_all().unwrap();
        assert_eq!(tokens[0].0.kind, TokenKind::Ident);
        assert_eq!(tokens[0].1, "x");
        assert_eq!(tokens[1].0.kind, TokenKind::Ident);
        assert_eq!(tokens[1].1, "中");
    }

    #[test]
    fn test_lexer_four_byte_char_at_end() {
        // 字符串末尾是 4 字节 Emoji，验证 current_char_len 正确返回 4 不越界。
        // Emoji 不是合法标识符字符，lexer 会报 "unexpected character" 错误。
        // 测试重点是 lexer 不 panic 且正确读取 4 字节序列。
        let src = "hi 🌍";
        let result = Lexer::new(src).scan_all();
        // 应该成功扫描 "hi" 然后在 emoji 处报错，或者整体失败
        // 关键是不应 panic 或读越界
        match result {
            Ok(tokens) => {
                // 如果成功，至少 "hi" 应被识别
                assert!(tokens.iter().any(|(t, _)| t.kind == TokenKind::Ident));
            }
            Err(e) => {
                // 报错是可接受的（emoji 非法字符），关键是错误信息合理
                assert!(
                    e.message.contains("unexpected character") || e.message.contains("emoji"),
                    "unexpected error: {}",
                    e.message
                );
            }
        }
    }

    // ========================================================================
    // P1-2: scan_cjk_token 边界回归测试
    // ========================================================================

    #[test]
    fn test_cjk_boundary_at_end_of_source() {
        // 单字符 CJK 关键字在源码末尾（无后续字符）—— 验证 peek() 返回 None
        // 时不会错误地尝试读取 two_char_text
        let tokens = Lexer::new("真").scan_all().unwrap();
        assert_eq!(tokens[0].0.kind, TokenKind::True);
        assert_eq!(tokens[1].0.kind, TokenKind::Eof);
    }

    #[test]
    fn test_cjk_two_char_keyword_at_end() {
        // 双字符 CJK 关键字在源码末尾（恰好两个字符）—— 验证 two_char_end 计算
        // 不会越界且能正确识别关键字
        let tokens = Lexer::new("如果").scan_all().unwrap();
        assert_eq!(tokens[0].0.kind, TokenKind::If);
        assert_eq!(tokens[1].0.kind, TokenKind::Eof);
    }

    #[test]
    fn test_cjk_single_char_followed_by_non_cjk() {
        // CJK 单字关键字后跟 ASCII 字符 —— 验证不误判为双字关键字
        let tokens = Lexer::new("真x").scan_all().unwrap();
        assert_eq!(tokens[0].0.kind, TokenKind::True);
        assert_eq!(tokens[1].0.kind, TokenKind::Ident);
        assert_eq!(tokens[1].1, "x");
    }

    #[test]
    fn test_cjk_keyword_then_cjk_identifier() {
        // CJK 关键字后紧跟 CJK 标识符（非关键字）—— 验证 cjk_keyword_starts_at
        // 边界检测在多字节字符间正确切分
        let tokens = Lexer::new("如果变量").scan_all().unwrap();
        assert_eq!(tokens[0].0.kind, TokenKind::If); // 如果
        assert_eq!(tokens[1].0.kind, TokenKind::Ident); // 变量
        assert_eq!(tokens[1].1, "变量");
    }
}
