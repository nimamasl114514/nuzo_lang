//! 源码位置扩展 trait，统一源码位置操作。
//!
//! `nuzo_core::SourceLocation` 已提供 `new()`, `with_column()`,
//! `with_function()`, `with_source_line()` 等链式构造方法，
//! 以及 `Display` trait（格式为 `file:line:column (in function fn)`）。
//!
//! 此 trait 补充 `SourceLocation` 未提供的功能：
//! - `is_unknown()` — 判断是否为默认/未知位置
//! - `to_compact_string()` — 紧凑格式（file:line[:column]，不含函数名）
//! - `format_context()` — 带源码行的上下文格式

use nuzo_core::SourceLocation;

/// 源码位置扩展 trait。
pub trait SourceLocationExt {
    /// 判断是否为默认/未知位置。
    ///
    /// 当 `file == "<unknown>"` 且 `line == 0` 时返回 `true`，
    /// 对应 `SourceLocation::default()` 的值。
    fn is_unknown(&self) -> bool;

    /// 返回紧凑的位置字符串：`file:line[:column]`。
    ///
    /// 与 `Display` trait 的区别：
    /// - 不包含函数名（`Display` 会附加 `(in function fn)`）
    /// - 适用于 IDE 跳转、日志等只需位置信息的场景
    fn to_compact_string(&self) -> String;

    /// 格式化源码上下文信息。
    ///
    /// 返回格式：`file:line[:column]`，若有源码行内容则附加换行和内容。
    /// 与 `Display`/`to_compact_string` 的区别在于此方法包含 `source_line` 内容，
    /// 适用于终端错误输出等需要完整上下文的场景。
    fn format_context(&self) -> String;
}

impl SourceLocationExt for SourceLocation {
    fn is_unknown(&self) -> bool {
        self.file == "<unknown>" && self.line == 0
    }

    fn to_compact_string(&self) -> String {
        if self.column > 0 {
            format!("{}:{}:{}", self.file, self.line, self.column)
        } else {
            format!("{}:{}", self.file, self.line)
        }
    }

    fn format_context(&self) -> String {
        let mut ctx = self.to_compact_string();
        if let Some(ref line) = self.source_line {
            ctx.push_str(&format!("\n  {}", line));
        }
        ctx
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_unknown_default() {
        let loc = SourceLocation::default();
        assert!(loc.is_unknown(), "default SourceLocation should be unknown");
    }

    #[test]
    fn test_is_unknown_new_with_line() {
        let loc = SourceLocation::new(10);
        assert!(
            !loc.is_unknown(),
            "new(10) has line=10, so it's not unknown despite file=<unknown>"
        );
    }

    #[test]
    fn test_is_unknown_with_file_and_line() {
        let loc = SourceLocation {
            file: "test.nu".to_string(),
            line: 10,
            column: 5,
            source_line: None,
            function_name: None,
        };
        assert!(!loc.is_unknown(), "non-default location should not be unknown");
    }

    #[test]
    fn test_is_unknown_zero_line_unknown_file() {
        let loc = SourceLocation {
            file: "<unknown>".to_string(),
            line: 0,
            column: 0,
            source_line: None,
            function_name: None,
        };
        assert!(loc.is_unknown(), "file=<unknown> + line=0 should be unknown");
    }

    #[test]
    fn test_to_compact_string_with_column() {
        let loc = SourceLocation {
            file: "test.nu".to_string(),
            line: 10,
            column: 5,
            source_line: None,
            function_name: None,
        };
        assert_eq!(loc.to_compact_string(), "test.nu:10:5");
    }

    #[test]
    fn test_to_compact_string_without_column() {
        let loc = SourceLocation {
            file: "test.nu".to_string(),
            line: 10,
            column: 0,
            source_line: None,
            function_name: None,
        };
        assert_eq!(loc.to_compact_string(), "test.nu:10");
    }

    #[test]
    fn test_to_compact_string_no_function_name() {
        let loc = SourceLocation {
            file: "test.nu".to_string(),
            line: 10,
            column: 5,
            source_line: None,
            function_name: Some("main".to_string()),
        };
        // to_compact_string should NOT include function name (unlike Display)
        assert_eq!(loc.to_compact_string(), "test.nu:10:5");
        assert!(!loc.to_compact_string().contains("main"));
    }

    #[test]
    fn test_format_context_with_source_line() {
        let loc = SourceLocation {
            file: "test.nu".to_string(),
            line: 10,
            column: 5,
            source_line: Some("let x = 1".to_string()),
            function_name: None,
        };
        assert_eq!(loc.format_context(), "test.nu:10:5\n  let x = 1");
    }

    #[test]
    fn test_format_context_without_source_line() {
        let loc = SourceLocation {
            file: "test.nu".to_string(),
            line: 10,
            column: 5,
            source_line: None,
            function_name: None,
        };
        assert_eq!(loc.format_context(), "test.nu:10:5");
    }
}
