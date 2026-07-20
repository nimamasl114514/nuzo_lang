//! # Nuzo Frontend — Nuzo 词法分析与语法分析前端
//!
//! **层级**: L4（前端层）—— 将源码转换为抽象语法树（AST），是编译器 pipeline 的第一阶段。
//!
//! **主要入口**: [`Lexer`], [`Parser`], [`Program`], [`Expr`], [`Stmt`], [`Token`], [`TokenKind`]
//!
//! ## 解析流水线
//!
//! ```text
//! 源码 (String)
//!   ↓ [lexer]   字符 → Token（状态机扫描）
//!   ↓ [parser]  Token → AST（递归下降 + 优先级 climbing）
//!   → Program (AST)
//! ```
//!
//! ## 模块职责
//!
//! | 模块 | 文件 | 职责 | 入口类型 |
//! |------|------|------|----------|
//! | [`token`] | token.rs | Token 类型定义、双语关键字映射 | [`Token`](token::Token), [`TokenKind`](token::TokenKind) |
//! | [`lexer`] | lexer.rs | 词法分析器、CJK 感知扫描 | [`Lexer`](lexer::Lexer) |
//! | [`ast`] | ast.rs | 抽象语法树节点类型系统 | [`Program`](ast::Program), [`Expr`](ast::Expr), [`Stmt`](ast::Stmt) |
//! | [`parser`] | parser.rs | 递归下降语法分析器（主入口） | [`Parser`](parser::Parser) |
//!
//! ## 开发者速查：常见任务 → 代码位置
//!
//! | 任务 | 位置 |
//! |------|------|
//! | "加新 Token 类型" | `token.rs: TokenKind 枚举` + `lexer.rs: 扫描分支` |
//! | "加新关键字（中/英）" | `token.rs: KEYWORDS 表` |
//! | "加新 AST 节点" | `ast.rs: Expr/Stmt 枚举` + `parser.rs: 解析函数` |
//! | "改运算符优先级" | `parser.rs: 表达式解析函数调用顺序` |
//! | "改错误恢复策略" | `parser.rs: synchronize()` |

// Crate 元数据——外层属性形式（`#![inner_attr]` 在 stable Rust 不稳定）
#[nuzo_proc::crate_meta(layer = 4, description = "词法分析与语法解析", entry_type = "Parser")]
const _NUZO_CRATE_META_ANCHOR: () = ();

pub mod ast;
pub mod lexer;
pub mod parser;
pub mod token;

pub use token::{Token, TokenKind};

pub use lexer::Lexer;

pub use ast::{BinaryOp, Expr, Program, Span, Stmt, UnaryOp};

pub use parser::ParseTimings;
pub use parser::Parser;
