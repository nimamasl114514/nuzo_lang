//! 统一诊断渲染器
//!
//! 将 [`DiagnosticError`]、[`NuzoError`] 与 [`CompileError`] 渲染为一致的终端/文本格式。
//! 复用 [`DiagnosticFormatter`] 处理颜色、宽度与分隔线，保证与现有 `ErrorCollector`
//! 的输出风格兼容。

use std::fmt::Write as FmtWrite;

use nuzo_abi::source_ext::SourceLocationExt;
use nuzo_core::error::ErrorCode;
use nuzo_core::{InternalError, LangMode, NuzoError, SourceLocation};

use crate::classifier::ErrorClassifier;
use crate::diagnostic::DiagnosticError;
use crate::formatter::DiagnosticFormatter;
use crate::types::{
    ErrorCategory, ErrorSeverity, ExecutionContext, StackFrameInfo, StructuredSuggestion,
};

// ============================================================================
// DiagnosticRenderer
// ============================================================================

/// 统一的诊断渲染器。
///
/// 支持链式配置颜色与宽度，并能为编译期、运行期和内部错误生成一致的报告。
#[derive(Debug, Clone)]
pub struct DiagnosticRenderer {
    formatter: DiagnosticFormatter,
    /// 语言模式，影响修复建议（suggestion）的文案语言。
    /// 默认从 `NUZO_LANG` 环境变量读取，可用 [`with_lang`](Self::with_lang) 覆盖。
    lang: LangMode,
    /// 可选的完整源码（按行分割），用于在错误位置渲染多行上下文 snippet。
    ///
    /// 通过 [`with_source_context`](Self::with_source_context) 注入；
    /// 未注入时仅渲染 `SourceLocation.source_line`（单行）。
    source_lines: Option<Vec<String>>,
    /// 错误行上下文行数（前后各显示多少行）。默认 1 行。
    context_radius: usize,
    /// 当前作用域可用变量名列表，用于 UndefinedVariable 错误的 "Did you mean X?" 拼写纠错建议。
    ///
    /// 通过 [`with_candidates`](Self::with_candidates) 注入；
    /// 为空时退回到基础建议（不做拼写纠错），保持向后兼容。
    candidates: Vec<String>,
}

impl DiagnosticRenderer {
    /// 使用自动检测的终端宽度与颜色支持创建渲染器。
    ///
    /// 语言模式从 `NUZO_LANG` 环境变量读取（默认 [`LangMode::Both`]）。
    pub fn new() -> Self {
        Self {
            formatter: DiagnosticFormatter::new(),
            lang: LangMode::from_env(),
            source_lines: None,
            context_radius: 1,
            candidates: Vec::new(),
        }
    }

    /// 创建一个禁用颜色但保留当前宽度的渲染器。
    pub fn no_color(self) -> Self {
        Self { formatter: self.formatter.with_color(false), ..self }
    }

    /// 强制设置颜色开关，保留其他配置。
    ///
    /// 传 `true` 启用 ANSI 颜色（用于强制着色，即使终端不支持），
    /// 传 `false` 等价于 [`no_color`](Self::no_color)。
    pub fn with_color(self, colorize: bool) -> Self {
        Self { formatter: self.formatter.with_color(colorize), ..self }
    }

    /// 创建一个使用指定宽度（自动 clamp 到 [60, 120]）的渲染器，颜色设置保留。
    pub fn with_width(self, width: usize) -> Self {
        Self { formatter: self.formatter.with_width(width), ..self }
    }

    /// 设置渲染器的语言模式，影响修复建议的文案语言。
    ///
    /// 默认从 `NUZO_LANG` 环境变量读取；调用此方法可显式覆盖（例如在测试中固定语言）。
    pub fn with_lang(self, lang: LangMode) -> Self {
        Self { lang, ..self }
    }

    /// 注入完整源码（按行分割存储），启用多行上下文 snippet 渲染。
    ///
    /// 当渲染 [`SourceLocation`] 时，若错误行号落在 `source` 的行范围内，
    /// 渲染器会显示错误行前后各 `context_radius` 行（默认 1 行）的上下文，
    /// 类似 `rustc` 的诊断输出：
    ///
    /// ```text
    /// --> file.nu:10:5
    ///    |
    ///  9 | let y = 0
    /// 10 | let x = a / b
    ///    |     ^
    /// 11 | print(x)
    ///    |
    /// ```
    ///
    /// 不注入则退回到单行渲染（仅 `source_line` + `^` 指针）。
    pub fn with_source_context(self, source: &str) -> Self {
        let lines: Vec<String> = source.lines().map(|s| s.to_string()).collect();
        Self { source_lines: Some(lines), ..self }
    }

    /// 设置上下文 snippet 的"半径"——错误行前后各显示多少行。默认 1。
    ///
    /// 设为 0 则只显示错误行本身（行为退化为单行渲染，但保留 `|` 分隔格式）。
    pub fn with_context_radius(self, radius: usize) -> Self {
        Self { context_radius: radius, ..self }
    }

    /// 注入当前作用域可用变量名列表，用于在 [`NuzoErrorKind::UndefinedVariable`]
    /// 错误下生成 "Did you mean 'X'?" 拼写纠错建议。
    ///
    /// 候选列表通常由 REPL、IDE 插件、或 CLI 的 `--suggest-vars` 选项从当前
    /// 作用域的变量名集合中提供。未注入（默认空 `Vec`）时退回到基础建议，
    /// 不做拼写纠错，保持向后兼容。
    ///
    /// 建议数量上限为 3，按 Levenshtein 距离升序排列；距离超过
    /// [`similar::default_max_distance`](crate::similar::default_max_distance)
    /// 的候选会被过滤掉。
    pub fn with_candidates(self, candidates: Vec<String>) -> Self {
        Self { candidates, ..self }
    }

    /// 当前渲染器使用的语言模式。
    pub fn lang(&self) -> LangMode {
        self.lang
    }

    /// 按当前语言模式选择 UI 字符串：`(中文, 英文) -> 对应文案`。
    ///
    /// 用于把渲染器内部硬编码的 UI 标题（如"修复建议"、"调用栈"）
    /// 也跟随 `--lang` 切换。
    fn tr(&self, zh: &str, en: &str) -> String {
        self.lang.select(zh, en)
    }

    /// 根据错误码前缀返回对应的样式（用于 `[CODE]` 部分着色）。
    ///
    /// 错误码分类与样式映射：
    ///
    /// | 前缀 | 类别 | 样式 | 视觉 |
    /// |------|------|------|------|
    /// | `E`  | 运行时错误（TypeMismatch/DivisionByZero 等） | [`fatal_style`](DiagnosticFormatter::fatal_style) | 粗体红 |
    /// | `C`  | 编译期错误（CompileError/ModuleNotFound 等） | [`compile_style`](DiagnosticFormatter::compile_style) | 粗体蓝 |
    /// | `I`  | 内部错误（Internal/InvalidBytecodeVersion） | [`internal_style`](DiagnosticFormatter::internal_style) | 粗体黄 |
    ///
    /// 通过错误码颜色一眼区分错误来源：红=用户代码、蓝=编译器、黄=VM/编译器 bug。
    fn error_code_style(&self, code: ErrorCode) -> crate::formatter::AnsiStyle {
        let prefix = error_code_str(code).chars().next().unwrap_or('E');
        match prefix {
            'C' => self.formatter.compile_style(),
            'I' => self.formatter.internal_style(),
            _ => self.formatter.fatal_style(),
        }
    }

    /// 渲染一个完整的 [`DiagnosticError`]。
    pub fn render_diagnostic(&self, diagnostic: &DiagnosticError) -> String {
        let mut output = String::new();

        // 优先使用 NuzoError（信息更完整），否则退回到兼容字段。
        let (code, message) = if let Some(ref nuzo_err) = diagnostic.nuzo_error {
            (nuzo_err.code(), format!("{}", nuzo_err))
        } else {
            (diagnostic.error.code(), format!("{}", diagnostic.error))
        };
        let severity = diagnostic.severity;

        // 标题行：[CODE] emoji 标签 #id
        // [CODE] 部分用 error_code_style（按错误类别着色），
        // 其余部分用 severity_style（按严重级别着色），形成"码-级"双色对比。
        let code_part = self.error_code_style(code).apply_to(format!("[{}]", error_code_str(code)));
        let rest_part = self.formatter.severity_style(severity).apply_to(format!(
            " {} {} #{}",
            self.formatter.severity_emoji(severity),
            self.formatter.severity_label(severity),
            diagnostic.id
        ));
        let _ = writeln!(output, "{}{}", code_part, rest_part);

        // 错误消息
        let _ = writeln!(output, "{}", self.formatter.error_style().apply_to(message));

        // 源码位置：优先 context，其次 NuzoError，最后兼容字段
        let loc = diagnostic
            .context
            .source_location
            .as_ref()
            .or(diagnostic.error.source_location.as_ref())
            .or_else(|| diagnostic.nuzo_error.as_ref().and_then(|e| e.source_location.as_ref()));
        if let Some(loc) = loc {
            self.render_source_location(&mut output, loc);
        }

        // 调用栈
        if !diagnostic.call_stack.is_empty() {
            self.render_stack_trace(&mut output, &diagnostic.call_stack);
        }

        // VM 诊断报告（仅内部错误）
        if let Some(ref diag) = diagnostic.diagnosis {
            let title = self.tr("VM 诊断报告", "VM Diagnosis Report");
            let _ = writeln!(output, "{}", self.formatter.section_header("🔬", &title));
            let _ = write!(output, "{}", diag);
        }

        // 修复建议
        self.render_suggestions(
            &mut output,
            &diagnostic.structured_suggestions,
            &diagnostic.fix_suggestions,
        );

        output
    }

    /// 渲染一个 [`NuzoError`]，可附带调用栈。
    pub fn render_nuzo_error(&self, error: &NuzoError, stack: &[StackFrameInfo]) -> String {
        let (severity, category) = ErrorClassifier::classify(error);
        let source_location = error.source_location.clone();

        let mut context = ExecutionContext::new(0, None, stack.len());
        if let Some(loc) = source_location {
            context.source_location(loc);
        }

        let diagnostic = DiagnosticError {
            id: 0,
            error: error.clone(),
            severity,
            category,
            context,
            call_stack: stack.to_vec(),
            instruction_count: 0,
            fix_suggestions: ErrorClassifier::generate_fix_suggestion_with_lang(error, self.lang),
            structured_suggestions:
                ErrorClassifier::generate_structured_suggestions_with_candidates(
                    error,
                    self.lang,
                    &self.candidates,
                ),
            nuzo_error: Some(error.clone()),
            diagnosis: None,
        };

        self.render_diagnostic(&diagnostic)
    }

    /// 渲染一个编译错误消息，使用提供的源码位置。
    pub fn render_compile_error(&self, message: &str, loc: SourceLocation) -> String {
        let nuzo_error =
            NuzoError::internal(InternalError::CompilerBug { message: message.to_string() }, None)
                .with_code(ErrorCode::CompileError);

        let mut context = ExecutionContext::new(0, None, 0);
        context.source_location(loc);

        let diagnostic = DiagnosticError {
            id: 0,
            error: nuzo_error.clone(),
            severity: ErrorSeverity::Error,
            category: ErrorCategory::Internal,
            context,
            call_stack: Vec::new(),
            instruction_count: 0,
            fix_suggestions: Vec::new(),
            structured_suggestions: Vec::new(),
            nuzo_error: Some(nuzo_error),
            diagnosis: None,
        };

        self.render_diagnostic(&diagnostic)
    }

    // ========================================================================
    // 内部辅助方法
    // ========================================================================

    fn render_source_location(&self, output: &mut String, loc: &SourceLocation) {
        let location_text = format!("--> {}", loc.to_compact_string());
        let _ = writeln!(output, "{}", self.formatter.cyan_style().apply_to(location_text));

        // 若注入了完整源码且错误行落在范围内，渲染多行上下文 snippet。
        // 否则退回到单行渲染（source_line + ^ 指针）。
        if let Some(ref lines) = self.source_lines
            && loc.line > 0
            && loc.line <= lines.len()
        {
            self.render_context_snippet(output, lines, loc.line, loc.column);
            return;
        }

        // 单行回退路径
        if let Some(ref line) = loc.source_line {
            let _ = writeln!(output, "{}", line);
            if loc.column > 0 {
                let spaces = " ".repeat(loc.column.saturating_sub(1));
                let pointer = self.formatter.error_style().apply_to("^");
                let _ = writeln!(output, "{}{}", spaces, pointer);
            }
        }
    }

    /// 渲染多行上下文 snippet（Rust 风格）。
    ///
    /// 格式示例（错误行 10、列 5、radius=1）：
    /// ```text
    ///    |
    ///  9 | let y = 0
    /// 10 | let x = a / b
    ///    |     ^
    /// 11 | print(x)
    ///    |
    /// ```
    fn render_context_snippet(
        &self,
        output: &mut String,
        lines: &[String],
        err_line: usize,
        err_column: usize,
    ) {
        let total = lines.len();
        let radius = self.context_radius;
        let start = err_line.saturating_sub(radius).max(1);
        let end = (err_line + radius).min(total);

        // 计算行号显示宽度（对齐用）
        let max_line_num = end;
        let gutter_width = max_line_num.to_string().len();

        // 顶部分隔线
        let _ = writeln!(output, "{:width$} |", "", width = gutter_width);

        for n in start..=end {
            let line_idx = n - 1; // 0-based
            let content = &lines[line_idx];
            if n == err_line {
                // 错误行用 error_style 着色
                let styled = self.formatter.error_style().apply_to(content.clone());
                let _ = writeln!(output, "{:>width$} | {}", n, styled, width = gutter_width);
                // 指针行
                if err_column > 0 {
                    let leading = " ".repeat(err_column.saturating_sub(1));
                    let pointer = self.formatter.error_style().apply_to("^");
                    let _ = writeln!(
                        output,
                        "{:width$} | {}{}",
                        "",
                        leading,
                        pointer,
                        width = gutter_width
                    );
                }
            } else {
                let _ = writeln!(output, "{:>width$} | {}", n, content, width = gutter_width);
            }
        }

        // 底部分隔线
        let _ = writeln!(output, "{:width$} |", "", width = gutter_width);
    }

    fn render_stack_trace(&self, output: &mut String, stack: &[StackFrameInfo]) {
        let title = self.tr("调用栈", "Call Stack");
        let _ = writeln!(output, "{}", self.formatter.section_header("🔄", &title));
        for (i, frame) in stack.iter().rev().enumerate() {
            let _ = writeln!(output, "  #{} {}", i, frame);
        }
    }

    fn render_suggestions(
        &self,
        output: &mut String,
        structured: &[StructuredSuggestion],
        legacy: &[String],
    ) {
        if structured.is_empty() && legacy.is_empty() {
            return;
        }

        let title = self.tr("修复建议", "Fix Suggestions");
        let _ = writeln!(output, "{}", self.formatter.section_header("💡", &title));

        let replace_label = self.tr("替换为", "Replace with");
        let location_label = self.tr("位置", "Location");
        let mut index = 1usize;
        for suggestion in structured {
            let _ = writeln!(output, "  {}. {}", index, suggestion.message);
            if let Some(ref replacement) = suggestion.replacement {
                let styled = self
                    .formatter
                    .success_style()
                    .apply_to(format!("     {}: {}", replace_label, replacement));
                let _ = writeln!(output, "{}", styled);
            }
            if let Some(ref span) = suggestion.span {
                let styled = self.formatter.cyan_style().apply_to(format!(
                    "     {}: {}",
                    location_label,
                    span.to_compact_string()
                ));
                let _ = writeln!(output, "{}", styled);
            }
            index += 1;
        }

        for suggestion in legacy {
            let _ = writeln!(output, "  {}. {}", index, suggestion);
            index += 1;
        }
    }
}

impl Default for DiagnosticRenderer {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// ErrorCode -> 字符串
// ============================================================================

fn error_code_str(code: ErrorCode) -> &'static str {
    match code {
        ErrorCode::TypeMismatch => "E0001",
        ErrorCode::IndexOutOfBounds => "E0002",
        ErrorCode::DivisionByZero => "E0003",
        ErrorCode::ArithmeticOverflow => "E0004",
        ErrorCode::AssertFailed => "E0005",
        ErrorCode::ExpectedNumber => "E0006",
        ErrorCode::InvalidArgumentCount => "E0007",
        ErrorCode::UndefinedVariable => "E0008",
        ErrorCode::UnsupportedOperation => "E0009",
        ErrorCode::ExecutionTimeout => "E0010",
        ErrorCode::CompileError => "C0000",
        ErrorCode::ModuleNotFound => "C0001",
        ErrorCode::CircularImport => "C0002",
        ErrorCode::DuplicateSymbol => "C0004",
        ErrorCode::SyntaxError => "C0005",
        ErrorCode::Internal => "I0000",
        ErrorCode::InvalidBytecodeVersion => "I0100",
    }
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn renderer() -> DiagnosticRenderer {
        DiagnosticRenderer::new().no_color().with_width(80)
    }

    // ------------------------------------------------------------------
    // Basic render with source line and pointer
    // ------------------------------------------------------------------

    #[test]
    fn render_basic_source_line_and_pointer() {
        let err = NuzoError::division_by_zero().with_source_location(SourceLocation {
            file: "test.nu".to_string(),
            line: 10,
            column: 5,
            source_line: Some("let x = a / b".to_string()),
            function_name: None,
        });

        let out = renderer().render_nuzo_error(&err, &[]);

        assert!(out.contains("[E0003]"), "应包含错误码 [E0003]\n{}", out);
        assert!(out.contains("division by zero"), "应包含错误消息\n{}", out);
        assert!(out.contains("test.nu:10:5"), "应包含源码位置\n{}", out);
        assert!(out.contains("let x = a / b"), "应包含源代码行\n{}", out);
        assert!(out.contains("    ^"), "应在第 5 列显示 ^ 指针\n{}", out);
    }

    // ------------------------------------------------------------------
    // Render without column pointer
    // ------------------------------------------------------------------

    #[test]
    fn render_without_column_pointer() {
        let err = NuzoError::index_out_of_bounds("10", "5").with_source_location(SourceLocation {
            file: "test.nu".to_string(),
            line: 20,
            column: 0,
            source_line: Some("arr[idx]".to_string()),
            function_name: None,
        });

        let out = renderer().render_nuzo_error(&err, &[]);

        assert!(out.contains("test.nu:20"), "应包含源码位置\n{}", out);
        assert!(out.contains("arr[idx]"), "应包含源代码行\n{}", out);
        assert!(!out.contains("    ^"), "列号为 0 时不应绘制 ^ 指针\n{}", out);
    }

    // ------------------------------------------------------------------
    // Render with stack trace
    // ------------------------------------------------------------------

    #[test]
    fn render_with_stack_trace() {
        let err = NuzoError::division_by_zero();

        let mut main_frame = StackFrameInfo::new("main".to_string(), 0);
        main_frame.source("main.nu".to_string(), 1);

        let mut divide_frame = StackFrameInfo::new("divide".to_string(), 8);
        divide_frame.source("math.nu".to_string(), 15);
        divide_frame.call_site(SourceLocation {
            file: "main.nu".to_string(),
            line: 10,
            column: 5,
            source_line: None,
            function_name: None,
        });

        let out = renderer().render_nuzo_error(&err, &[main_frame, divide_frame]);

        assert!(out.contains("调用栈"), "应包含调用栈标题\n{}", out);
        assert!(out.contains("main"), "应包含 main 帧\n{}", out);
        assert!(out.contains("divide"), "应包含 divide 帧\n{}", out);
    }

    // ------------------------------------------------------------------
    // Render with suggestions
    // ------------------------------------------------------------------

    #[test]
    fn render_with_suggestions() {
        let err = NuzoError::type_mismatch("number".to_string(), "string".to_string());

        let out = renderer().render_nuzo_error(&err, &[]);

        assert!(out.contains("修复建议"), "应包含修复建议标题\n{}", out);
        assert!(
            out.contains("expected number, got string") || out.contains("check the value type"),
            "应包含结构化建议内容\n{}",
            out
        );
    }

    // ------------------------------------------------------------------
    // no_color mode
    // ------------------------------------------------------------------

    #[test]
    fn render_no_color_mode_strips_ansi() {
        let err = NuzoError::division_by_zero();

        let colored = DiagnosticRenderer::new().with_width(80).render_nuzo_error(&err, &[]);
        let plain =
            DiagnosticRenderer::new().no_color().with_width(80).render_nuzo_error(&err, &[]);

        assert!(!plain.contains("\x1b["), "no_color 模式不应包含 ANSI 转义码\n{}", plain);
        assert!(colored.contains("division by zero"), "带颜色模式仍应包含错误消息\n{}", colored);
    }

    // ------------------------------------------------------------------
    // CompileError rendering uses the same format
    // ------------------------------------------------------------------

    #[test]
    fn render_compile_error_uses_unified_format() {
        let message = "[line 3] undefined variable 'x'".to_string();
        let loc = SourceLocation {
            file: "prog.nu".to_string(),
            line: 3,
            column: 7,
            source_line: Some("print(x)".to_string()),
            function_name: None,
        };

        let out = renderer().render_compile_error(&message, loc);

        assert!(out.contains("[C0000]"), "编译错误应使用 [C0000] 码\n{}", out);
        assert!(out.contains("undefined variable"), "应包含编译错误消息\n{}", out);
        assert!(out.contains("prog.nu:3:7"), "应包含源码位置\n{}", out);
        assert!(out.contains("print(x)"), "应包含源代码行\n{}", out);
    }

    // ------------------------------------------------------------------
    // with_lang: 渲染器根据语言模式切换建议文案
    // ------------------------------------------------------------------

    #[test]
    fn render_with_lang_zh_uses_chinese_suggestion() {
        let err = NuzoError::division_by_zero();

        let out_zh = DiagnosticRenderer::new()
            .no_color()
            .with_width(80)
            .with_lang(LangMode::Zh)
            .render_nuzo_error(&err, &[]);

        assert!(out_zh.contains("修复建议"), "中文模式应包含修复建议标题\n{}", out_zh);
        assert!(out_zh.contains("除法前确保除数不为零"), "中文模式应包含中文建议文案\n{}", out_zh);
        assert!(
            !out_zh.contains("Check for zero divisors"),
            "中文模式不应包含英文建议\n{}",
            out_zh
        );
    }

    #[test]
    fn render_with_lang_en_uses_english_suggestion() {
        let err = NuzoError::division_by_zero();

        let out_en = DiagnosticRenderer::new()
            .no_color()
            .with_width(80)
            .with_lang(LangMode::En)
            .render_nuzo_error(&err, &[]);

        assert!(out_en.contains("Fix Suggestions"), "英文模式应使用英文标题\n{}", out_en);
        assert!(
            out_en.contains("ensure the divisor is not zero before dividing"),
            "英文模式应包含英文建议文案\n{}",
            out_en
        );
        assert!(
            out_en.contains("Replace with:"),
            "英文模式应使用英文标签 'Replace with'\n{}",
            out_en
        );
        assert!(!out_en.contains("除法前确保除数不为零"), "英文模式不应包含中文建议\n{}", out_en);
        assert!(!out_en.contains("修复建议"), "英文模式不应包含中文 UI 标题\n{}", out_en);
    }

    #[test]
    fn render_with_lang_both_emits_bilingual_suggestion() {
        let err = NuzoError::division_by_zero();

        let out_both = DiagnosticRenderer::new()
            .no_color()
            .with_width(80)
            .with_lang(LangMode::Both)
            .render_nuzo_error(&err, &[]);

        assert!(out_both.contains("除法前确保除数不为零"), "Both 模式应包含中文建议\n{}", out_both);
        assert!(
            out_both.contains("ensure the divisor is not zero before dividing"),
            "Both 模式应包含英文建议\n{}",
            out_both
        );
        assert!(
            out_both.contains("修复建议") && out_both.contains("Fix Suggestions"),
            "Both 模式应同时包含中英文 UI 标题\n{}",
            out_both
        );
    }

    #[test]
    fn render_with_lang_default_is_from_env() {
        let renderer = DiagnosticRenderer::new();
        assert_eq!(renderer.lang(), LangMode::from_env(), "new() 默认应从 NUZO_LANG 读取语言模式");
    }

    // ------------------------------------------------------------------
    // with_source_context: 多行上下文 snippet 渲染
    // ------------------------------------------------------------------

    /// 构造测试用源码（5 行）：
    /// ```text
    /// 1: let a = 1
    /// 2: let b = 2
    /// 3: let x = a / b
    /// 4: print(x)
    /// 5: end
    /// ```
    fn sample_source() -> &'static str {
        "let a = 1\nlet b = 2\nlet x = a / b\nprint(x)\nend\n"
    }

    #[test]
    fn render_source_context_shows_surrounding_lines() {
        let err = NuzoError::division_by_zero().with_source_location(SourceLocation {
            file: "test.nu".to_string(),
            line: 3,
            column: 5,
            source_line: None,
            function_name: None,
        });

        let out = DiagnosticRenderer::new()
            .no_color()
            .with_width(80)
            .with_source_context(sample_source())
            .render_nuzo_error(&err, &[]);

        // 上下文各 1 行（radius=1 默认）。行号 1-4 均为 1 位数，故无前导空格。
        assert!(out.contains("2 | let b = 2"), "应显示错误行上方 1 行\n{}", out);
        assert!(out.contains("3 | let x = a / b"), "应显示错误行本身\n{}", out);
        assert!(out.contains("4 | print(x)"), "应显示错误行下方 1 行\n{}", out);
        // ^ 指针应在错误行下方第 5 列
        assert!(out.contains("    ^"), "应在第 5 列绘制 ^ 指针\n{}", out);
        // 不应显示第 1 行和第 5 行（不在 radius=1 范围内）
        assert!(!out.contains("1 | let a = 1"), "radius=1 时不应显示第 1 行\n{}", out);
        assert!(!out.contains("5 | end"), "radius=1 时不应显示第 5 行\n{}", out);
    }

    #[test]
    fn render_source_context_radius_2_shows_more_lines() {
        let err = NuzoError::division_by_zero().with_source_location(SourceLocation {
            file: "test.nu".to_string(),
            line: 3,
            column: 5,
            source_line: None,
            function_name: None,
        });

        let out = DiagnosticRenderer::new()
            .no_color()
            .with_width(80)
            .with_source_context(sample_source())
            .with_context_radius(2)
            .render_nuzo_error(&err, &[]);

        // radius=2 应显示 1-5 行
        assert!(out.contains("1 | let a = 1"), "radius=2 应显示第 1 行\n{}", out);
        assert!(out.contains("2 | let b = 2"), "radius=2 应显示第 2 行\n{}", out);
        assert!(out.contains("3 | let x = a / b"), "radius=2 应显示错误行\n{}", out);
        assert!(out.contains("4 | print(x)"), "radius=2 应显示第 4 行\n{}", out);
        assert!(out.contains("5 | end"), "radius=2 应显示第 5 行\n{}", out);
    }

    #[test]
    fn render_source_context_fallback_without_source_lines() {
        // 未注入 source_lines 时，退回单行渲染
        let err = NuzoError::division_by_zero().with_source_location(SourceLocation {
            file: "test.nu".to_string(),
            line: 3,
            column: 5,
            source_line: Some("let x = a / b".to_string()),
            function_name: None,
        });

        let out = renderer().render_nuzo_error(&err, &[]);

        // 单行渲染不应有 "|" 分隔符
        assert!(out.contains("let x = a / b"), "应显示单行 source_line\n{}", out);
        assert!(out.contains("    ^"), "应在第 5 列绘制 ^ 指针\n{}", out);
        assert!(!out.contains(" | "), "未注入 source 时不应渲染多行 snippet\n{}", out);
    }

    #[test]
    fn render_source_context_handles_first_line() {
        // 错误在第 1 行：上方无行，下方应显示 radius 行
        let err = NuzoError::division_by_zero().with_source_location(SourceLocation {
            file: "test.nu".to_string(),
            line: 1,
            column: 1,
            source_line: None,
            function_name: None,
        });

        let out = DiagnosticRenderer::new()
            .no_color()
            .with_width(80)
            .with_source_context(sample_source())
            .render_nuzo_error(&err, &[]);

        assert!(out.contains("1 | let a = 1"), "应显示第 1 行\n{}", out);
        assert!(out.contains("2 | let b = 2"), "应显示第 2 行\n{}", out);
        assert!(!out.contains("0 |"), "不应显示第 0 行（不存在）\n{}", out);
    }

    #[test]
    fn render_source_context_handles_last_line() {
        // 错误在最后一行：下方无行
        let err = NuzoError::division_by_zero().with_source_location(SourceLocation {
            file: "test.nu".to_string(),
            line: 5,
            column: 1,
            source_line: None,
            function_name: None,
        });

        let out = DiagnosticRenderer::new()
            .no_color()
            .with_width(80)
            .with_source_context(sample_source())
            .render_nuzo_error(&err, &[]);

        assert!(out.contains("4 | print(x)"), "应显示第 4 行\n{}", out);
        assert!(out.contains("5 | end"), "应显示第 5 行\n{}", out);
        assert!(!out.contains("6 |"), "不应显示第 6 行（不存在）\n{}", out);
    }

    #[test]
    fn render_source_context_clamps_out_of_range_line() {
        // 错误行超出源码范围：退回单行渲染
        let err = NuzoError::division_by_zero().with_source_location(SourceLocation {
            file: "test.nu".to_string(),
            line: 999,
            column: 1,
            source_line: Some("missing".to_string()),
            function_name: None,
        });

        let out = DiagnosticRenderer::new()
            .no_color()
            .with_width(80)
            .with_source_context(sample_source())
            .render_nuzo_error(&err, &[]);

        // 行号超出范围时退回单行渲染
        assert!(out.contains("missing"), "应退回显示 source_line\n{}", out);
        assert!(!out.contains(" | "), "行号超出范围时不应渲染 snippet\n{}", out);
    }

    // ------------------------------------------------------------------
    // with_candidates：UndefinedVariable 拼写纠错端到端
    // ------------------------------------------------------------------

    #[test]
    fn render_with_candidates_shows_did_you_mean_en() {
        // English: UndefinedVariable + candidates → "Did you mean 'count'?"
        let err = NuzoError::undefined_variable("conut").with_source_location(SourceLocation {
            file: "test.nu".to_string(),
            line: 1,
            column: 1,
            source_line: Some("print(conut)".to_string()),
            function_name: None,
        });

        let out = DiagnosticRenderer::new()
            .no_color()
            .with_width(80)
            .with_lang(LangMode::En)
            .with_candidates(vec!["count".to_string(), "counter".to_string()])
            .render_nuzo_error(&err, &[]);

        assert!(out.contains("[E0008]"), "应包含 UndefinedVariable 错误码\n{}", out);
        assert!(
            out.contains("Did you mean 'count'?"),
            "应包含英文 'Did you mean' 建议而非中文\n{}",
            out
        );
        assert!(out.contains("Replace with: count"), "应包含 replacement 行\n{}", out);
    }

    #[test]
    fn render_with_candidates_shows_did_you_mean_zh() {
        // 中文: UndefinedVariable + candidates → "你是否想用 'count'？"
        let err = NuzoError::undefined_variable("conut").with_source_location(SourceLocation {
            file: "test.nu".to_string(),
            line: 1,
            column: 1,
            source_line: Some("print(conut)".to_string()),
            function_name: None,
        });

        let out = DiagnosticRenderer::new()
            .no_color()
            .with_width(80)
            .with_lang(LangMode::Zh)
            .with_candidates(vec!["count".to_string()])
            .render_nuzo_error(&err, &[]);

        assert!(out.contains("你是否想用 'count'"), "应包含中文 '你是否想用' 建议\n{}", out);
    }

    #[test]
    fn render_without_candidates_no_did_you_mean() {
        // 未注入 candidates 时不渲染 "Did you mean"（向后兼容）
        let err = NuzoError::undefined_variable("conut").with_source_location(SourceLocation {
            file: "test.nu".to_string(),
            line: 1,
            column: 1,
            source_line: Some("print(conut)".to_string()),
            function_name: None,
        });

        let out = renderer().render_nuzo_error(&err, &[]);

        assert!(!out.contains("Did you mean"), "未注入 candidates 时不应渲染拼写纠错建议\n{}", out);
        assert!(
            !out.contains("你是否想用"),
            "未注入 candidates 时不应渲染中文拼写纠错建议\n{}",
            out
        );
    }

    // ------------------------------------------------------------------
    // 错误码颜色高亮（E0xxx 红 / C0xxx 蓝 / I0xxx 黄）
    // ------------------------------------------------------------------

    // ANSI 码常量（与 formatter.rs 内部常量保持同步）
    const ANSI_BOLD: &str = "\x1b[1m";
    const ANSI_FG_RED: &str = "\x1b[31m";
    const ANSI_FG_BLUE: &str = "\x1b[34m";
    const ANSI_FG_YELLOW: &str = "\x1b[33m";

    fn color_renderer() -> DiagnosticRenderer {
        DiagnosticRenderer::new().with_color(true).with_width(80)
    }

    #[test]
    fn render_error_code_color_runtime_red() {
        // E0xxx（运行时错误）→ 粗体红
        let err = NuzoError::division_by_zero();
        let out = color_renderer().render_nuzo_error(&err, &[]);

        assert!(out.contains("[E0003]"), "应包含错误码 [E0003]\n{}", out);
        assert!(out.contains(ANSI_FG_RED), "E0xxx 错误码应使用红色（FG_RED）\n{}", out);
        assert!(out.contains(ANSI_BOLD), "E0xxx 错误码应使用粗体（BOLD）\n{}", out);
    }

    #[test]
    fn render_error_code_color_compile_blue() {
        // C0xxx（编译期错误）→ 粗体蓝
        let err = NuzoError::internal(
            InternalError::CompilerBug { message: "syntax error".to_string() },
            None,
        )
        .with_code(ErrorCode::CompileError);
        let out = color_renderer().render_nuzo_error(&err, &[]);

        assert!(out.contains("[C0000]"), "应包含错误码 [C0000]\n{}", out);
        assert!(out.contains(ANSI_FG_BLUE), "C0xxx 错误码应使用蓝色（FG_BLUE）\n{}", out);
        assert!(out.contains(ANSI_BOLD), "C0xxx 错误码应使用粗体（BOLD）\n{}", out);
        // 编译期错误不应使用红色或黄色
        assert!(
            !out.contains(ANSI_FG_RED) || out.find(ANSI_FG_RED) > out.find(ANSI_FG_BLUE),
            "C0xxx 错误码首现的应为蓝色而非红色\n{}",
            out
        );
    }

    #[test]
    fn render_error_code_color_internal_yellow() {
        // I0xxx（内部错误）→ 粗体黄
        let err = NuzoError::internal(InternalError::NoChunkLoaded, None);
        let out = color_renderer().render_nuzo_error(&err, &[]);

        assert!(out.contains("[I0000]"), "应包含错误码 [I0000]\n{}", out);
        assert!(out.contains(ANSI_FG_YELLOW), "I0xxx 错误码应使用黄色（FG_YELLOW）\n{}", out);
        assert!(out.contains(ANSI_BOLD), "I0xxx 错误码应使用粗体（BOLD）\n{}", out);
    }

    #[test]
    fn render_error_code_no_color_passthrough() {
        // no_color 模式下 [CODE] 不应包含任何 ANSI 码
        let err = NuzoError::division_by_zero();
        let out = renderer().render_nuzo_error(&err, &[]);

        assert!(out.contains("[E0003]"), "应包含错误码 [E0003]\n{}", out);
        assert!(!out.contains(ANSI_FG_RED), "no_color 模式不应包含红色码\n{}", out);
        assert!(!out.contains(ANSI_FG_BLUE), "no_color 模式不应包含蓝色码\n{}", out);
        assert!(!out.contains(ANSI_FG_YELLOW), "no_color 模式不应包含黄色码\n{}", out);
        assert!(!out.contains(ANSI_BOLD), "no_color 模式不应包含粗体码\n{}", out);
    }

    #[test]
    fn render_error_code_colors_distinct_between_categories() {
        // 三类错误码颜色互不相同：E 红、C 蓝、I 黄
        let e_err = NuzoError::division_by_zero();
        let c_err =
            NuzoError::internal(InternalError::CompilerBug { message: "x".to_string() }, None)
                .with_code(ErrorCode::CompileError);
        let i_err = NuzoError::internal(InternalError::NoChunkLoaded, None);

        let r = color_renderer();
        let e_out = r.render_nuzo_error(&e_err, &[]);
        let c_out = r.render_nuzo_error(&c_err, &[]);
        let i_out = r.render_nuzo_error(&i_err, &[]);

        // E 用红，不用蓝/黄
        assert!(e_out.contains(ANSI_FG_RED));
        // C 用蓝
        assert!(c_out.contains(ANSI_FG_BLUE));
        // I 用黄
        assert!(i_out.contains(ANSI_FG_YELLOW));
    }
}
