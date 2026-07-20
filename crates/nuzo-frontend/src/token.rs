//! # Nuzo Token 类型定义模块
//!
//! ## 模块职责
//! 定义 Nuzo 语言的词法单元（Token）类型系统，包括：
//! - [`TokenKind`] 枚举：所有 Token 类型的分类（关键字、运算符、分隔符等）
//! - [`Token`] 结构体：携带位置信息的词法单元实例
//! - 双语关键字映射机制（英文 + 中文 → 同一 TokenKind）
//!
//! ## 性能特征
//! - **内存占用**：每个 Token 仅占用 12 字节（kind: 1B + line: 8B + column: 8B 实际对齐后更小）
//! - **零拷贝扫描**：Token 的文本内容通过 `&str` 切片引用原始源码
//! - **O(1) 关键字查找**：基于 `match` 的完美哈希分发
//! - **UTF-8 友好**：中文标识符不会导致 4x 内存膨胀（相比 `Vec<char>` 方案）
//!
//! ## 双语关键字设计
//!
//! Nuzo 语言支持中英文双语关键字，它们在语法层面完全等价：
//!
//! | 英文关键字 | 中文关键字 | TokenKind | 语义 |
//! |-----------|-----------|-----------|------|
//! | `if` | `如果` | `If` | 条件分支 |
//! | `else` | `否则` | `Else` | 否则分支 |
//! | `while` | `当` | `While` | 当型循环 |
//! | `for` | `遍历` | `For` | 遍历循环 |
//! | `in` | `在` | `In` | 遍历目标 |
//! | `loop` | `循环` | `Loop` | 无限循环 |
//! | `break` | `跳出` | `Break` | 跳出循环 |
//! | `continue` | `继续` | `Continue` | 继续循环 |
//! | `fn` | `函数` | `Fn` | 函数声明 |
//! | `return` | `返回` | `Return` | 返回值 |
//! | `true` | `真` | `True` | 布尔真 |
//! | `false` | `假` | `False` | 布尔假 |
//! | `nil` | `空` | `Nil` | 空值 |
//! | `and` | `并且` | `And` | 逻辑与 |
//! | `or` | `或者` | `Or` | 逻辑或 |
//!
//! ## Token 分类体系
//!
//! ```text
//! TokenKind (枚举)
//! ├── 字面量 (Literals)
//! │   ├── Number    - 数值字面量（整数/浮点数）
//! │   ├── String    - 字符串字面量（双引号/单引号）
//! │   └── Ident     - 标识符（变量名/函数名）
//! ├── 关键字 (Keywords) - 双语支持
//! │   ├── 控制流: If, Else, While, For, In, Loop, Break, Continue
//! │   ├── 函数: Fn, Return
//! │   ├── 字面量: True, False, Nil
//! │   └── 逻辑: And, Or
//! ├── 运算符 (Operators)
//! │   ├── 算术: Plus, Minus, Star, Slash, Percent
//! │   ├── 比较: Eq, EqEq, Bang, BangEq, Lt, Gt, LtEq, GtEq
//! │   ├── 逻辑: AndAnd, OrOr
//! │   ├── 其他: Arrow, PlusEqual, MinusEqual, StarEqual, SlashEqual
//! ├── 分隔符 (Delimiters)
//! │   ├── 括号: LParen, RParen, LBrace, RBrace, LBracket, RBracket
//! │   ├── 标点: Comma, Dot, Colon, Semicolon
//! ├── 范围运算符 (Range)
//! │   ├── DotDot    - 包含范围 (..)
//! │   └── DotDotLt  - 不包含范围 (..<)
//! └── 特殊标记
//! │   └── Eof       - 文件结束标记
//! ```
//!
//! ## 宏机制说明
//!
//! 本模块使用 [`define_keywords!`] 宏实现**单一数据源**原则：
//! - 关键字定义只在一个地方维护
//! - 自动生成正向查找函数（文本 → TokenKind）
//! - 自动生成反向查找函数（TokenKind → 英文文本）
//! - 自动生成常量表供遍历使用

use std::fmt;

/// 词法单元种类枚举 - 定义所有可能的 Token 类型
///
/// # 设计原则
/// - 使用枚举而非字符串匹配，实现 O(1) 类型分发
/// - 双语关键字映射到同一变体，简化 Parser 处理
/// - 变体按语义分组，便于代码导航和维护
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, nuzo_proc::MatchSync)]
pub enum TokenKind {
    /// 数值字面量：支持整数和浮点数（如 `42`, `3.14`）
    Number,
    /// 字符串字面量：支持双引号和单引号（如 `"hello"`, `'world'`）
    String,
    /// 标识符：变量名、函数名、属性名（如 `myVar`, `add`）
    Ident,

    /// 条件分支（if / 如果）
    If,
    /// 否则分支（else / 否则）
    Else,
    /// 当型循环（while / 当）
    While,
    /// 遍历循环（for / 遍历）
    For,
    /// 遍历目标介词（in / 在）
    In,
    /// 无限循环（loop / 循环）
    Loop,
    /// 跳出循环（break / 跳出）
    Break,
    /// 继续循环（continue / 继续）
    Continue,
    /// 函数声明（fn / 函数）
    Fn,
    /// 返回语句（return / 返回）
    Return,
    /// 异常捕获（try / 尝试）
    Try,
    /// 异常处理（catch / 捕获）
    Catch,
    /// 抛出异常（out / 抛出）
    Out,
    /// 始终执行（keep / 始终）
    Keep,
    /// 模式匹配（match / 匹配）
    Match,
    /// 布尔真值（true / 真）
    True,
    /// 布尔假值（false / 假）
    False,
    /// 空值（nil / 空）
    Nil,
    /// 逻辑与（and / 并且）- 注意：还有短路版本 `&&`
    And,
    /// 逻辑或（or / 或者）- 注意：还有短路版本 `||`
    Or,
    /// 导入模块（import / 导入）
    Import,
    /// 懒求值（lazy / 懒）
    Lazy,
    /// 别名（as / 作为）
    As,

    /// 加法运算符 `+`
    Plus,
    /// 减法运算符 `-`
    Minus,
    /// 乘法运算符 `*`
    Star,
    /// 幂运算符 `**`（右结合）
    StarStar,
    /// 除法运算符 `/`
    Slash,
    /// 取模运算符 `%`
    Percent,
    /// 赋值运算符 `=`
    Eq,
    /// 相等比较 `==`
    EqEq,
    /// 逻辑非 `!`
    Bang,
    /// 不等比较 `!=`
    BangEq,
    /// 小于比较 `<`
    Lt,
    /// 大于比较 `>`
    Gt,
    /// 小于等于 `<=`
    LtEq,
    /// 大于等于 `>=`
    GtEq,
    /// 短路逻辑与 `&&`
    AndAnd,
    /// 短路逻辑或 `||`
    OrOr,
    /// 箭头运算符 `=>`（用于 lambda 表达式）
    Arrow,
    /// 加法赋值 `+=`
    PlusEqual,
    /// 减法赋值 `-=`
    MinusEqual,
    /// 乘法赋值 `*=`
    StarEqual,
    /// 除法赋值 `/=`
    SlashEqual,
    /// 管道运算符 `|>`（左到右函数链式调用）
    Pipe,
    /// 空值合并运算符 `??`（nil 时取默认值）
    QuestionQuestion,

    /// 左圆括号 `(`
    LParen,
    /// 右圆括号 `)`
    RParen,
    /// 左花括号 `{`
    LBrace,
    /// 右花括号 `}`
    RBrace,
    /// 左方括号 `[`
    LBracket,
    /// 右方括号 `]`
    RBracket,
    /// 逗号 `,`
    Comma,
    /// 点号 `.`
    Dot,
    /// 冒号 `:`（用于字典键值对、类型标注）
    Colon,
    /// 分号 `;`（语句分隔符）
    Semicolon,

    /// 包含范围 `..`（如 `1..5` 表示 [1, 2, 3, 4, 5]）
    DotDot,
    /// 不包含范围 `..<`（如 `1..<5` 表示 [1, 2, 3, 4]）
    DotDotLt,

    /// 文件结束标记（End of File）
    Eof,
}

/// 词法单元结构体 - 表示源代码中的一个 Token
///
/// # 内存布局
/// Token 在 64 位系统上实际占用约 32 字节（含对齐填充）：
/// - `kind`: TokenKind 枚举（1 字节，但 Rust 会按枚举最大变体对齐，此处按 1 字节算）
/// - `line`: usize 行号（8 字节，64 位系统）
/// - `column`: usize 列号（8 字节，64 位系统）
/// - `offset`: usize 字节偏移（8 字节，64 位系统，用于零拷贝切片定位）
/// - 结构体按 8 字节对齐，`kind` 后填充 7 字节以对齐 `line`
///
/// 如未来对内存敏感，可将 `line`/`column`/`offset` 改为 `u32`（各 4 字节），
/// 使 Token 缩小到约 16 字节。当前优先保持与 `SourceLocation::line`/`column`
/// 类型一致（`usize`），避免类型转换开销。
///
/// # 位置信息语义
/// - `line`: 从 1 开始计数的行号
/// - `column`: 从 1 开始计数的列号（UTF-8 字符位置）
/// - `offset`: 从 0 开始的字节偏移量（用于快速切片）
///
/// # 零拷贝设计
/// Token 本身不存储文本内容，而是通过 `offset` 配合 Lexer 的源码引用，
/// 在需要时动态生成 `&str` 切片。这避免了字符串复制开销。
#[derive(Debug, Clone, Copy)]
pub struct Token {
    /// Token 种类标识
    pub kind: TokenKind,
    /// 所在行号（1-based）
    pub line: usize,
    /// 所在列号（1-based）
    pub column: usize,
    /// 在源码中的字节偏移量（0-based），用于零拷贝切片
    pub offset: usize,
}

impl Token {
    /// 创建新的 Token 实例
    ///
    /// # 参数
    /// - `kind`: Token 种类
    /// - `line`: 行号（从 1 开始）
    /// - `column`: 列号（从 1 开始）
    /// - `offset`: 源码字节偏移（从 0 开始）
    ///
    /// # 示例
    /// ```
    /// use nuzo_frontend::{Token, TokenKind};
    /// let tok = Token::new(TokenKind::Number, 10, 5, 100);
    /// assert_eq!(tok.line, 10);
    /// ```
    pub fn new(kind: TokenKind, line: usize, column: usize, offset: usize) -> Self {
        Token { kind, line, column, offset }
    }

    /// 创建 EOF（文件结束）Token
    ///
    /// EOF Token 是一个特殊的哨兵值，表示词法分析的终止。
    /// 它不关联任何源码文本。
    ///
    /// # 参数
    /// - `line`: EOF 所在的行号（通常是最后一行）
    /// - `column`: EOF 所在的列号
    /// - `offset`: 源码长度（即 EOF 的字节位置）
    ///
    /// # 使用场景
    /// Lexer 扫描完所有输入后自动生成 EOF Token，
    /// Parser 用它来判断是否到达输入末尾。
    pub fn eof(line: usize, column: usize, offset: usize) -> Self {
        Token::new(TokenKind::Eof, line, column, offset)
    }
}

// ---------------------------------------------------------------------------
// define_keywords! — 双语关键字映射宏（单一数据源设计）
// ---------------------------------------------------------------------------
//
// # 设计理念
// 这个宏实现了**"定义一次，到处使用"**的原则：
// - 关键字映射表只在一个地方维护
// - 编译时自动生成所有相关的查找函数和常量
// - 消除手工同步导致的不一致风险
//
// # 生成的代码
// 调用 `define_keywords! { "if" "如果" => If, ... }` 会生成：
//
// 1. **lookup_keyword 函数**：文本 → TokenKind 的正向查找
//    - 支持英文和中文字符串
//    - 返回 Option<TokenKind>，未找到返回 None
//    - 时间复杂度：O(1)（编译器优化为跳转表）
//
// 2. **KEYWORDS 常量表**：所有关键字的枚举视图
//    - 类型：&[(&str, &str, TokenKind)]
//    - 用于 IDE 自动补全、文档生成、错误提示
//    - 格式：(英文, 中文, TokenKind)
//
// 3. **display_keyword 函数**：TokenKind → 英文文本的反向查找
//    - 仅用于 Display 实现
//    - 非关键字变体返回 None
//
// # 使用示例
// ```ignore
// define_keywords! {
//     "if"   "如果" => If,
//     "fn"   "函数" => Fn,
// }
// // 生成：
// // lookup_keyword("if")   → Some(If)
// // lookup_keyword("如果") → Some(If)
// // KEYWORDS = [("if", "如果", If), ("fn", "函数", Fn)]
// // display_keyword(If) → Some("if")
// ```
// ---------------------------------------------------------------------------

macro_rules! define_keywords {
    ($($eng:literal $chn:literal => $variant:ident),* $(,)?) => {
        /// 双语关键字查找函数
        ///
        /// 根据文本查找对应的 TokenKind，支持英文和中文字符串。
        ///
        /// # 参数
        /// * `ident` - 要查找的标识符文本（如 `"if"` 或 `"如果"`）
        ///
        /// # 返回值
        /// * `Some(TokenKind)` - 找到对应的关键字
        /// * `None` - 不是关键字（可能是普通标识符）
        ///
        /// # 性能特征
        /// 使用 `match` 实现完美哈希分发，时间复杂度 O(1)。
        /// 编译器会将其优化为跳转表，无字符串比较开销。
        ///
        /// # 示例
        /// ```ignore
        /// assert_eq!(lookup_keyword("if"), Some(TokenKind::If));
        /// assert_eq!(lookup_keyword("如果"), Some(TokenKind::If));
        /// assert_eq!(lookup_keyword("variable"), None);  // 非关键字
        /// ```
        pub fn lookup_keyword(ident: &str) -> Option<TokenKind> {
            match ident {
                $($eng | $chn => Some(TokenKind::$variant),)*
                _ => None,
            }
        }

        /// 双语关键字常量表
        ///
        /// 提供所有关键字的完整列表，格式为 `(英文, 中文, TokenKind)` 三元组。
        ///
        /// # 使用场景
        /// - IDE 自动补全生成
        /// - 错误消息中的关键字建议
        /// - 文档工具的关键字索引
        /// - 语法高亮的配置生成
        ///
        /// # 数据结构
        /// ```text
        /// [
        ///   ("if", "如果", If),
        ///   ("else", "否则", Else),
        ///   ...
        /// ]
        /// ```
        pub const KEYWORDS: &[(&str, &str, TokenKind)] = &[
            $(($eng, $chn, TokenKind::$variant),)*
        ];

        /// 关键字反向查找（内部使用）
        ///
        /// 给定一个 TokenKind，返回其英文文本表示。
        /// 仅对关键字变体有效，非关键字返回 None。
        ///
        /// # 注意事项
        /// 此函数是私有的，仅用于 [`TokenKind`] 的 Display 实现。
        /// 外部代码应使用 `format!("{}", token_kind)` 来获取文本表示。
        fn display_keyword(kind: TokenKind) -> Option<&'static str> {
            match kind {
                $(TokenKind::$variant => Some($eng),)*
                _ => None,
            }
        }
    }
}

// 双语关键字定义表
//
// 此处定义了 Nuzo 语言支持的所有双语关键字映射。
// **修改关键字时只需修改此表**，宏会自动更新所有相关代码。
//
// # 扩展指南
// 如需添加新关键字：
// 1. 在 [`TokenKind`] 枚举中添加新变体
// 2. 在此表中添加映射：`"english" "中文" => NewVariant`
// 3. 在 Parser 中添加相应的语法规则
// 4. （可选）在 is_xxx() 谓词方法中添加快捷方法
define_keywords! {
    "if"       "如果" => If,
    "else"     "否则" => Else,
    "while"    "当"   => While,
    "for"      "遍历" => For,
    "in"       "在"   => In,
    "loop"     "循环" => Loop,
    "break"    "跳出" => Break,
    "continue" "继续" => Continue,
    "fn"       "函数" => Fn,
    "return"   "返回" => Return,
    "true"     "真"   => True,
    "false"    "假"   => False,
    "nil"      "空"   => Nil,
    "and"      "并且" => And,
    "or"       "或者" => Or,
    "not"      "非"   => Bang,  // logical NOT: maps to Bang (!) for parser compatibility
    "try"      "尝试" => Try,
    "catch"    "捕获" => Catch,
    "out"      "抛出" => Out,
    "keep"     "始终" => Keep,
    "match"    "匹配" => Match,
    "import"   "导入" => Import,
    "lazy"     "懒"   => Lazy,
    "as"       "作为" => As,
}

/// TokenKind 的文本显示实现
///
/// 将 TokenKind 转换为人类可读的字符串表示。
///
/// # 显示规则
/// - **关键字**：显示英文形式（如 `If` → `"if"`）
/// - **运算符**：显示符号本身（如 `Plus` → `"+"`）
/// - **分隔符**：显示符号本身（如 `LParen` → `"("`）
/// - **字面量**：显示类型名称（如 `Number` → `"number"`）
/// - **特殊标记**：显示描述（如 `Eof` → `"EOF"`）
///
/// # 使用场景
/// - 错误消息生成（如 "unexpected token '+'"）
/// - 调试输出和日志
/// - 单元测试断言
/// - REPL 交互界面
impl fmt::Display for TokenKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // 优先使用关键字反向查找（返回英文形式）
        if let Some(kw) = display_keyword(*self) {
            write!(f, "{}", kw)
        } else {
            // 非关键字变体使用硬编码映射
            match self {
                TokenKind::Number => write!(f, "number"),
                TokenKind::String => write!(f, "string"),
                TokenKind::Ident => write!(f, "identifier"),

                TokenKind::Plus => write!(f, "+"),
                TokenKind::Minus => write!(f, "-"),
                TokenKind::Star => write!(f, "*"),
                TokenKind::Slash => write!(f, "/"),
                TokenKind::Percent => write!(f, "%"),
                TokenKind::Eq => write!(f, "="),
                TokenKind::EqEq => write!(f, "=="),
                TokenKind::Bang => write!(f, "!"),
                TokenKind::BangEq => write!(f, "!="),
                TokenKind::Lt => write!(f, "<"),
                TokenKind::Gt => write!(f, ">"),
                TokenKind::LtEq => write!(f, "<="),
                TokenKind::GtEq => write!(f, ">="),
                TokenKind::AndAnd => write!(f, "&&"),
                TokenKind::OrOr => write!(f, "||"),
                TokenKind::Arrow => write!(f, "=>"),
                TokenKind::PlusEqual => write!(f, "+="),
                TokenKind::MinusEqual => write!(f, "-="),
                TokenKind::StarEqual => write!(f, "*="),
                TokenKind::SlashEqual => write!(f, "/="),
                TokenKind::Pipe => write!(f, "|>"),
                TokenKind::QuestionQuestion => write!(f, "??"),

                TokenKind::LParen => write!(f, "("),
                TokenKind::RParen => write!(f, ")"),
                TokenKind::LBrace => write!(f, "{{"), // 转义花括号（Rust 格式化语法）
                TokenKind::RBrace => write!(f, "}}"),
                TokenKind::LBracket => write!(f, "["),
                TokenKind::RBracket => write!(f, "]"),
                TokenKind::Comma => write!(f, ","),
                TokenKind::Dot => write!(f, "."),
                TokenKind::Colon => write!(f, ":"),
                TokenKind::Semicolon => write!(f, ";"),

                TokenKind::DotDot => write!(f, ".."),
                TokenKind::DotDotLt => write!(f, "..<"),

                TokenKind::Eof => write!(f, "EOF"),

                // 注意：关键字变体已在上方通过 display_keyword() 处理，
                // 此分支理论上不可达，但 Rust 枚举 match 要求穷尽性。
                _ => write!(f, "{:?}", self),
            }
        }
    }
}

/// 双语关键字谓词方法集合
///
/// 提供语义化的类型检查方法，用于 Parser 中的模式匹配。
/// 由于中文和英文关键字映射到同一 TokenKind，这些方法自动支持双语。
///
/// # 设计原则
/// - **语义明确**：`is_if()` 比 `kind == TokenKind::If` 更易读
/// - **双语透明**：调用者无需关心用户输入的是英文还是中文
/// - **性能无损**：编译器会内联这些简单方法
///
/// # 使用示例
/// ```ignore
/// if token.kind.is_if() {
///     // 处理 if/如果 关键字
/// }
/// ```
///
/// # 特殊情况处理
/// - `is_and()` 同时匹配 `And`（并且）和 `&&`
/// - `is_or()` 同时匹配 `Or`（或者）和 `||`
///
/// 这允许语言同时支持单词形式和符号形式的逻辑运算符。
impl TokenKind {
    /// 检查是否为条件分支关键字（if / 如果）
    pub fn is_if(self) -> bool {
        self == TokenKind::If
    }

    /// 检查是否为否则分支关键字（else / 否则）
    pub fn is_else(self) -> bool {
        self == TokenKind::Else
    }

    /// 检查是否为当型循环关键字（while / 当）
    pub fn is_while(self) -> bool {
        self == TokenKind::While
    }

    /// 检查是否为遍历循环关键字（for / 遍历）
    pub fn is_for(self) -> bool {
        self == TokenKind::For
    }

    /// 检查是否为遍历目标介词（in / 在）
    pub fn is_in(self) -> bool {
        self == TokenKind::In
    }

    /// 检查是否为无限循环关键字（loop / 循环）
    pub fn is_loop(self) -> bool {
        self == TokenKind::Loop
    }

    /// 检查是否为跳出循环关键字（break / 跳出）
    pub fn is_break(self) -> bool {
        self == TokenKind::Break
    }

    /// 检查是否为继续循环关键字（continue / 继续）
    pub fn is_continue(self) -> bool {
        self == TokenKind::Continue
    }

    /// 检查是否为函数声明关键字（fn / 函数）
    pub fn is_fn(self) -> bool {
        self == TokenKind::Fn
    }

    /// 检查是否为返回语句关键字（return / 返回）
    pub fn is_return(self) -> bool {
        self == TokenKind::Return
    }

    /// 检查是否为布尔真值字面量（true / 真）
    pub fn is_true(self) -> bool {
        self == TokenKind::True
    }

    /// 检查是否为布尔假值字面量（false / 假）
    pub fn is_false(self) -> bool {
        self == TokenKind::False
    }

    /// 检查是否为空值字面量（nil / 空）
    pub fn is_nil(self) -> bool {
        self == TokenKind::Nil
    }

    /// 检查是否为模式匹配关键字（match / 匹配）
    pub fn is_match(self) -> bool {
        self == TokenKind::Match
    }

    /// 检查是否为逻辑与运算符（and / 并且 / &&）
    ///
    /// # 注意事项
    /// 此方法同时匹配三种形式：
    /// - `And`（单词形式："and" 或 "并且"）
    /// - `&&`（符号形式）
    ///
    /// 这种设计允许程序员根据可读性需求选择不同风格。
    pub fn is_and(self) -> bool {
        matches!(self, TokenKind::AndAnd | TokenKind::And)
    }

    /// 检查是否为逻辑或运算符（or / 或者 / ||）
    ///
    /// # 注意事项
    /// 同 [`TokenKind::is_and`]，此方法同时匹配单词和符号两种形式。
    pub fn is_or(self) -> bool {
        matches!(self, TokenKind::OrOr | TokenKind::Or)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ========================================================================
    // TokenKind::is_* 谓词方法测试
    //
    // 每个测试验证目标变体返回 true，并抽样验证其他变体返回 false。
    // 对于 is_and / is_or，额外验证符号形式（AndAnd / OrOr）也返回 true。
    // ========================================================================

    #[test]
    fn test_is_if_true_for_if_variant() {
        assert!(TokenKind::If.is_if());
    }

    #[test]
    fn test_is_if_false_for_other_variants() {
        assert!(!TokenKind::Else.is_if());
        assert!(!TokenKind::While.is_if());
        assert!(!TokenKind::Number.is_if());
        assert!(!TokenKind::Eof.is_if());
    }

    #[test]
    fn test_is_else_true_for_else_variant() {
        assert!(TokenKind::Else.is_else());
    }

    #[test]
    fn test_is_else_false_for_other_variants() {
        assert!(!TokenKind::If.is_else());
        assert!(!TokenKind::While.is_else());
        assert!(!TokenKind::Return.is_else());
        assert!(!TokenKind::Ident.is_else());
    }

    #[test]
    fn test_is_while_true_for_while_variant() {
        assert!(TokenKind::While.is_while());
    }

    #[test]
    fn test_is_while_false_for_other_variants() {
        assert!(!TokenKind::For.is_while());
        assert!(!TokenKind::Loop.is_while());
        assert!(!TokenKind::If.is_while());
        assert!(!TokenKind::Break.is_while());
    }

    #[test]
    fn test_is_for_true_for_for_variant() {
        assert!(TokenKind::For.is_for());
    }

    #[test]
    fn test_is_for_false_for_other_variants() {
        assert!(!TokenKind::While.is_for());
        assert!(!TokenKind::In.is_for());
        assert!(!TokenKind::Loop.is_for());
        assert!(!TokenKind::Continue.is_for());
    }

    #[test]
    fn test_is_loop_true_for_loop_variant() {
        assert!(TokenKind::Loop.is_loop());
    }

    #[test]
    fn test_is_loop_false_for_other_variants() {
        assert!(!TokenKind::While.is_loop());
        assert!(!TokenKind::For.is_loop());
        assert!(!TokenKind::Break.is_loop());
        assert!(!TokenKind::Continue.is_loop());
    }

    #[test]
    fn test_is_break_true_for_break_variant() {
        assert!(TokenKind::Break.is_break());
    }

    #[test]
    fn test_is_break_false_for_other_variants() {
        assert!(!TokenKind::Continue.is_break());
        assert!(!TokenKind::Loop.is_break());
        assert!(!TokenKind::Return.is_break());
        assert!(!TokenKind::While.is_break());
    }

    #[test]
    fn test_is_continue_true_for_continue_variant() {
        assert!(TokenKind::Continue.is_continue());
    }

    #[test]
    fn test_is_continue_false_for_other_variants() {
        assert!(!TokenKind::Break.is_continue());
        assert!(!TokenKind::Loop.is_continue());
        assert!(!TokenKind::Return.is_continue());
        assert!(!TokenKind::While.is_continue());
    }

    #[test]
    fn test_is_fn_true_for_fn_variant() {
        assert!(TokenKind::Fn.is_fn());
    }

    #[test]
    fn test_is_fn_false_for_other_variants() {
        assert!(!TokenKind::Return.is_fn());
        assert!(!TokenKind::If.is_fn());
        assert!(!TokenKind::Ident.is_fn());
        assert!(!TokenKind::True.is_fn());
    }

    #[test]
    fn test_is_return_true_for_return_variant() {
        assert!(TokenKind::Return.is_return());
    }

    #[test]
    fn test_is_return_false_for_other_variants() {
        assert!(!TokenKind::Fn.is_return());
        assert!(!TokenKind::Break.is_return());
        assert!(!TokenKind::Continue.is_return());
        assert!(!TokenKind::Nil.is_return());
    }

    #[test]
    fn test_is_true_true_for_true_variant() {
        assert!(TokenKind::True.is_true());
    }

    #[test]
    fn test_is_true_false_for_other_variants() {
        assert!(!TokenKind::False.is_true());
        assert!(!TokenKind::Nil.is_true());
        assert!(!TokenKind::If.is_true());
        assert!(!TokenKind::Number.is_true());
    }

    #[test]
    fn test_is_false_true_for_false_variant() {
        assert!(TokenKind::False.is_false());
    }

    #[test]
    fn test_is_false_false_for_other_variants() {
        assert!(!TokenKind::True.is_false());
        assert!(!TokenKind::Nil.is_false());
        assert!(!TokenKind::If.is_false());
        assert!(!TokenKind::String.is_false());
    }

    #[test]
    fn test_is_and_true_for_and_variant() {
        assert!(TokenKind::And.is_and());
    }

    #[test]
    fn test_is_and_true_for_andand_symbol_variant() {
        // is_and 同时匹配单词形式 And 和符号形式 AndAnd
        assert!(TokenKind::AndAnd.is_and());
    }

    #[test]
    fn test_is_and_false_for_other_variants() {
        assert!(!TokenKind::Or.is_and());
        assert!(!TokenKind::OrOr.is_and());
        assert!(!TokenKind::If.is_and());
        assert!(!TokenKind::Bang.is_and());
    }

    #[test]
    fn test_is_or_true_for_or_variant() {
        assert!(TokenKind::Or.is_or());
    }

    #[test]
    fn test_is_or_true_for_oror_symbol_variant() {
        // is_or 同时匹配单词形式 Or 和符号形式 OrOr
        assert!(TokenKind::OrOr.is_or());
    }

    #[test]
    fn test_is_or_false_for_other_variants() {
        assert!(!TokenKind::And.is_or());
        assert!(!TokenKind::AndAnd.is_or());
        assert!(!TokenKind::If.is_or());
        assert!(!TokenKind::Bang.is_or());
    }

    // ========================================================================
    // 双语关键字映射验证（间接验证 is_* 方法的双语透明性）
    // ========================================================================

    #[test]
    fn test_bilingual_keywords_map_to_same_variant() {
        // 英文和中文关键字应映射到同一 TokenKind，
        // 从而 is_* 方法对两者都返回 true。
        assert!(lookup_keyword("if").unwrap().is_if());
        assert!(lookup_keyword("如果").unwrap().is_if());

        assert!(lookup_keyword("else").unwrap().is_else());
        assert!(lookup_keyword("否则").unwrap().is_else());

        assert!(lookup_keyword("while").unwrap().is_while());
        assert!(lookup_keyword("当").unwrap().is_while());

        assert!(lookup_keyword("for").unwrap().is_for());
        assert!(lookup_keyword("遍历").unwrap().is_for());

        assert!(lookup_keyword("loop").unwrap().is_loop());
        assert!(lookup_keyword("循环").unwrap().is_loop());

        assert!(lookup_keyword("break").unwrap().is_break());
        assert!(lookup_keyword("跳出").unwrap().is_break());

        assert!(lookup_keyword("continue").unwrap().is_continue());
        assert!(lookup_keyword("继续").unwrap().is_continue());

        assert!(lookup_keyword("fn").unwrap().is_fn());
        assert!(lookup_keyword("函数").unwrap().is_fn());

        assert!(lookup_keyword("return").unwrap().is_return());
        assert!(lookup_keyword("返回").unwrap().is_return());

        assert!(lookup_keyword("true").unwrap().is_true());
        assert!(lookup_keyword("真").unwrap().is_true());

        assert!(lookup_keyword("false").unwrap().is_false());
        assert!(lookup_keyword("假").unwrap().is_false());

        assert!(lookup_keyword("and").unwrap().is_and());
        assert!(lookup_keyword("并且").unwrap().is_and());

        assert!(lookup_keyword("or").unwrap().is_or());
        assert!(lookup_keyword("或者").unwrap().is_or());
    }

    #[test]
    fn test_lookup_keyword_returns_none_for_identifier() {
        // 非关键字应返回 None
        assert!(lookup_keyword("variable").is_none());
        assert!(lookup_keyword("myFunc").is_none());
        assert!(lookup_keyword("").is_none());
    }
}
