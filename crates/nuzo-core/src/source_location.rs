//! # 源码位置追踪类型
//!
//! 本模块定义 [`SourceLocation`] 结构体，用于**精确标记源代码中的位置**，
//! 是编译器错误报告和调试信息系统的核心数据结构。
//!
//! ## 设计目标
//!
//! ### 1. IDE 友好
//! 输出格式兼容大多数编辑器的 **"文件:行:列"** 跳转协议：
//! ```text
//! Error: undefined variable 'x' at script.nuzo:42:10
//!                              ^^^^^^^^^^ ^^^ ^^
//!                              文件      行  列
//! ```
//!
//! ### 2. 上下文丰富
//! 除了基本的位置信息，还支持携带 **出错行的源码快照**：
//! ```ignore
//! let loc = SourceLocation {
//!     file: "test.nuzo".to_string(),
//!     line: 10,
//!     column: 5,
//!     source_line: Some("    x = y + z".to_string()),
//! };
//!
//! // 格式化输出（用于错误报告）
//! println!("Error at {}", loc);
//! // Error at test.nuzo:10:5
//!
//! // 可选：展示带指针的错误行
//! if let Some(ref line) = loc.source_line {
//!     println!("{}", line);
//!     println!("{}^", " ".repeat(loc.column - 1));
//! }
//! //     x = y + z
//! //     ^
//! ```
//!
//! ### 3. 序列化支持
//! 实现 `serde::Serialize`，可集成到：
//! - JSON 格式的编译报告（CI/CD 使用）|
//! - Language Server Protocol (LSP) 诊断信息
//! - 测试快照文件（用于回归检测）
//!
//! ## 字段说明
//!
//! | 字段 | 类型 | 必填 | 说明 |
//! |------|------|------|------|
//! `file` | `String` | 是 | 源文件路径（绝对或相对）|
//! `line` | `usize` | 是 | 行号（从1开始，符合人类习惯）|
//! `column` | `usize` | 是 | 列号（从1开始，基于字节偏移量）|
//! `source_line` | `Option<String>` | 否 | 该行的源码内容（可选，增强可读性）|
//!
//! ## 默认值语义
//!
//! 当无法确定具体位置时（如运行时生成的代码），使用默认值：
//! - `file`: `"<unknown>"` — 表示来源不明
//! - `line`: `0` — 无效行号（IDE 会跳转到文件开头）|
//! - `column`: `0` — 无效列号
//! - `source_line`: `None` — 无源码上下文
//!
//! ## 使用场景
//!
//! ### 编译器前端
//! ```ignore
//! // 词法分析器/语法分析器在发现错误时创建
//! return Err(CompilerError {
//!     kind: ErrorKind::UnexpectedToken,
//!     location: SourceLocation {
//!         file: self.current_file().clone(),
//!         line: self.token.line,
//!         column: self.token.column,
//!         source_line: self.get_source_line(self.token.line),
//!     },
//! });
//! ```
//!
//! ### 运行时错误
//! ```ignore
//! // VM 在执行时通过 DebugInfo 反查源位置
//! let debug_info = chunk.debug_info();
//! let location = debug_info.source_location(pc);
//! return Err(NuzoError::new_with_location(
//!     NuzoErrorKind::IndexOutOfBounds,
//!     location,
//! ));
//! ```

use serde::Serialize;
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SourceLocation {
    pub file: String,
    pub line: usize,
    pub column: usize,
    pub source_line: Option<String>,
    /// 可选的 enclosing 函数名（用于错误报告中标注出错函数）
    pub function_name: Option<String>,
}

impl fmt::Display for SourceLocation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.file, self.line)?;
        if self.column > 0 {
            write!(f, ":{}", self.column)?;
        }
        if let Some(ref func) = self.function_name {
            write!(f, " (in function {})", func)?;
        }
        Ok(())
    }
}

impl Default for SourceLocation {
    fn default() -> Self {
        SourceLocation {
            file: "<unknown>".to_string(),
            line: 0,
            column: 0,
            source_line: None,
            function_name: None,
        }
    }
}

impl SourceLocation {
    /// 创建仅包含行号的源码位置（file 默认为 "<unknown>"）。
    ///
    /// 这是为了兼容旧版 `nuzo_values::errors::SourceLocation::new(line)` 的用法。
    pub fn new(line: usize) -> Self {
        SourceLocation {
            file: "<unknown>".to_string(),
            line,
            column: 0,
            source_line: None,
            function_name: None,
        }
    }

    /// 添加列号信息（链式调用）。
    pub fn with_column(mut self, column: usize) -> Self {
        self.column = column;
        self
    }

    /// 添加函数名信息（链式调用）。
    pub fn with_function(mut self, name: impl Into<String>) -> Self {
        self.function_name = Some(name.into());
        self
    }

    /// 添加源码行内容（链式调用）。
    pub fn with_source_line(mut self, line: impl Into<String>) -> Self {
        self.source_line = Some(line.into());
        self
    }
}
