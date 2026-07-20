//! 精确错误报告工具（span-aware 诊断）
//!
//! 提供 `Diagnostic` 构建器、`SpannedError`、`MultiDiagnostic` 等，
//! 支持 stable/nightly 双轨错误报告。
//!
//! ## 核心类型
//!
//! | 类型 | 用途 |
//! |------|------|
//! | [`DiagnosticLevel`] | 诊断级别（Error / Warning / Note / Help） |
//! | [`Diagnostic`] | 构建器模式诊断消息，支持 span、help、note 附加 |
//! | [`SpannedError`] | `syn::Error` 的薄封装，提供便捷构造器 |
//! | [`MultiDiagnostic`] | 多诊断聚合器，批量 emit / 转换为 Result |
//!
//! ## 快捷函数
//!
//! - [`error`] / [`error_at`] / [`warning`] — 一行创建诊断

use proc_macro2::Span;
use quote::quote;

// ---------------------------------------------------------------------------
// DiagnosticLevel
// ---------------------------------------------------------------------------

/// 诊断严重级别。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiagnosticLevel {
    Error,
    Warning,
    Note,
    Help,
}

// ---------------------------------------------------------------------------
// Diagnostic
// ---------------------------------------------------------------------------

/// 构建器模式诊断消息。
///
/// ```ignore
/// let d = Diagnostic::new(DiagnosticLevel::Error, "something went wrong")
///     .with_span(span)
///     .with_help("try fixing X");
/// let tokens = d.emit();
/// ```
#[derive(Clone, Debug)]
pub struct Diagnostic {
    level: DiagnosticLevel,
    message: String,
    span: Option<Span>,
    help: Option<String>,
    note: Option<String>,
}

impl Diagnostic {
    pub fn new(level: DiagnosticLevel, message: impl Into<String>) -> Self {
        Self { level, message: message.into(), span: None, help: None, note: None }
    }

    pub fn with_span(mut self, span: Span) -> Self {
        self.span = Some(span);
        self
    }

    pub fn with_help(mut self, help: impl Into<String>) -> Self {
        self.help = Some(help.into());
        self
    }

    pub fn with_note(mut self, note: impl Into<String>) -> Self {
        self.note = Some(note.into());
        self
    }

    /// 发射诊断：Error 级别生成 `compile_error!`，其余级别在 stable 下静默。
    pub fn emit(&self) -> proc_macro2::TokenStream {
        match self.level {
            DiagnosticLevel::Error => self.to_compile_error(),
            DiagnosticLevel::Warning | DiagnosticLevel::Note | DiagnosticLevel::Help => {
                // stable proc-macro 无法生成 warning/note，静默处理
                quote! {}
            }
        }
    }

    /// 无论级别如何，总是生成 `compile_error!`。
    pub fn to_compile_error(&self) -> proc_macro2::TokenStream {
        let msg = self.format_message();
        let span = self.span.unwrap_or_else(Span::call_site);
        syn::Error::new(span, msg).to_compile_error()
    }

    /// 访问器
    pub fn level(&self) -> &DiagnosticLevel {
        &self.level
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn span(&self) -> Option<Span> {
        self.span
    }

    pub fn help(&self) -> Option<&str> {
        self.help.as_deref()
    }

    pub fn note(&self) -> Option<&str> {
        self.note.as_deref()
    }

    fn format_message(&self) -> String {
        let mut parts = String::with_capacity(self.message.len() + 64);
        parts.push_str(&self.message);
        if let Some(ref note) = self.note {
            parts.push_str("\nnote: ");
            parts.push_str(note);
        }
        if let Some(ref help) = self.help {
            parts.push_str("\nhelp: ");
            parts.push_str(help);
        }
        parts
    }
}

// ---------------------------------------------------------------------------
// SpannedError
// ---------------------------------------------------------------------------

/// `syn::Error` 的薄封装，提供便捷构造器。
#[derive(Debug)]
pub struct SpannedError {
    inner: syn::Error,
}

impl SpannedError {
    pub fn new(span: Span, message: impl Into<String>) -> Self {
        let msg: String = message.into();
        Self { inner: syn::Error::new(span, msg) }
    }

    pub fn new_spanned(spanned: impl quote::ToTokens, message: impl Into<String>) -> Self {
        let msg: String = message.into();
        Self { inner: syn::Error::new_spanned(spanned, msg) }
    }

    pub fn to_compile_error(&self) -> proc_macro2::TokenStream {
        self.inner.to_compile_error()
    }

    pub fn into_inner(self) -> syn::Error {
        self.inner
    }
}

impl From<syn::Error> for SpannedError {
    fn from(err: syn::Error) -> Self {
        Self { inner: err }
    }
}

impl std::fmt::Display for SpannedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.inner.fmt(f)
    }
}

impl std::error::Error for SpannedError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.inner)
    }
}

// ---------------------------------------------------------------------------
// MultiDiagnostic
// ---------------------------------------------------------------------------

/// 多诊断聚合器，支持批量 emit / 转换为 `Result`。
#[derive(Clone, Debug, Default)]
pub struct MultiDiagnostic {
    diags: Vec<Diagnostic>,
}

impl MultiDiagnostic {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, diag: Diagnostic) {
        self.diags.push(diag);
    }

    pub fn is_empty(&self) -> bool {
        self.diags.is_empty()
    }

    pub fn has_errors(&self) -> bool {
        self.diags.iter().any(|d| d.level == DiagnosticLevel::Error)
    }

    /// 合并所有 Error 级别的 `compile_error!`，非 Error 级别在 stable 下静默。
    pub fn emit_all(&self) -> proc_macro2::TokenStream {
        self.diags
            .iter()
            .filter(|d| d.level == DiagnosticLevel::Error)
            .map(|d| d.to_compile_error())
            .collect()
    }

    /// 若存在 Error 级别诊断则返回 `Err`，否则 `Ok(())`。
    pub fn into_result(self) -> Result<(), Self> {
        if self.has_errors() { Err(self) } else { Ok(()) }
    }

    pub fn len(&self) -> usize {
        self.diags.len()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Diagnostic> {
        self.diags.iter()
    }
}

// ---------------------------------------------------------------------------
// 便捷函数
// ---------------------------------------------------------------------------

/// 快速创建 Error 级别诊断（无 span）。
pub fn error(msg: impl Into<String>) -> Diagnostic {
    Diagnostic::new(DiagnosticLevel::Error, msg)
}

/// 快速创建带 span 的 Error 级别诊断。
pub fn error_at(span: Span, msg: impl Into<String>) -> Diagnostic {
    Diagnostic::new(DiagnosticLevel::Error, msg).with_span(span)
}

/// 快速创建 Warning 级别诊断（无 span）。
pub fn warning(msg: impl Into<String>) -> Diagnostic {
    Diagnostic::new(DiagnosticLevel::Warning, msg)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use proc_macro2::Span;
    use std::error::Error;

    // -- DiagnosticLevel --

    #[test]
    fn diagnostic_level_equality() {
        assert_eq!(DiagnosticLevel::Error, DiagnosticLevel::Error);
        assert_ne!(DiagnosticLevel::Error, DiagnosticLevel::Warning);
    }

    // -- Diagnostic --

    #[test]
    fn diagnostic_new_basic() {
        let d = Diagnostic::new(DiagnosticLevel::Error, "oops");
        assert_eq!(d.level(), &DiagnosticLevel::Error);
        assert_eq!(d.message(), "oops");
        assert!(d.span().is_none());
        assert!(d.help().is_none());
        assert!(d.note().is_none());
    }

    #[test]
    fn diagnostic_builder_chaining() {
        let d = Diagnostic::new(DiagnosticLevel::Warning, "deprecated")
            .with_span(Span::call_site())
            .with_help("use new_thing instead")
            .with_note("since v2");
        assert!(d.span().is_some());
        assert_eq!(d.help(), Some("use new_thing instead"));
        assert_eq!(d.note(), Some("since v2"));
    }

    #[test]
    fn diagnostic_emit_error_produces_tokens() {
        let d = Diagnostic::new(DiagnosticLevel::Error, "bad input");
        let tokens = d.emit();
        let s = tokens.to_string();
        assert!(s.contains("compile_error"));
    }

    #[test]
    fn diagnostic_emit_warning_is_empty_on_stable() {
        let d = Diagnostic::new(DiagnosticLevel::Warning, "soft deprecation");
        let tokens = d.emit();
        assert!(tokens.is_empty());
    }

    #[test]
    fn diagnostic_emit_note_is_empty_on_stable() {
        let d = Diagnostic::new(DiagnosticLevel::Note, "fyi");
        let tokens = d.emit();
        assert!(tokens.is_empty());
    }

    #[test]
    fn diagnostic_to_compile_error_always_produces_tokens() {
        let d = Diagnostic::new(DiagnosticLevel::Warning, "should not happen");
        let tokens = d.to_compile_error();
        let s = tokens.to_string();
        assert!(s.contains("compile_error"));
    }

    #[test]
    fn diagnostic_format_message_with_note_and_help() {
        let d = Diagnostic::new(DiagnosticLevel::Error, "main msg")
            .with_note("context")
            .with_help("fix it");
        let formatted = d.format_message();
        assert!(formatted.contains("main msg"));
        assert!(formatted.contains("note: context"));
        assert!(formatted.contains("help: fix it"));
    }

    #[test]
    fn diagnostic_format_message_bare() {
        let d = Diagnostic::new(DiagnosticLevel::Error, "bare");
        let formatted = d.format_message();
        assert_eq!(formatted, "bare");
    }

    // -- SpannedError --

    #[test]
    fn spanned_error_new() {
        let err = SpannedError::new(Span::call_site(), "boom");
        let s = err.to_compile_error().to_string();
        assert!(s.contains("compile_error"));
    }

    #[test]
    fn spanned_error_new_spanned() {
        let lit: proc_macro2::TokenStream = quote! { 42 };
        let err = SpannedError::new_spanned(lit, "bad literal");
        let s = err.to_compile_error().to_string();
        assert!(s.contains("compile_error"));
    }

    #[test]
    fn spanned_error_from_syn_error() {
        let syn_err = syn::Error::new(Span::call_site(), "original");
        let spanned: SpannedError = syn_err.into();
        assert!(spanned.to_compile_error().to_string().contains("compile_error"));
    }

    #[test]
    fn spanned_error_into_inner() {
        let err = SpannedError::new(Span::call_site(), "inner test");
        let syn_err = err.into_inner();
        assert_eq!(syn_err.to_string(), "inner test");
    }

    #[test]
    fn spanned_error_display() {
        let err = SpannedError::new(Span::call_site(), "display me");
        let displayed = format!("{err}");
        assert!(displayed.contains("display me"));
    }

    #[test]
    fn spanned_error_error_trait() {
        let err = SpannedError::new(Span::call_site(), "source check");
        assert!(err.source().is_some());
    }

    // -- MultiDiagnostic --

    #[test]
    fn multi_diagnostic_new_is_empty() {
        let m = MultiDiagnostic::new();
        assert!(m.is_empty());
        assert!(!m.has_errors());
        assert_eq!(m.len(), 0);
    }

    #[test]
    fn multi_diagnostic_add_and_query() {
        let mut m = MultiDiagnostic::new();
        m.add(Diagnostic::new(DiagnosticLevel::Warning, "warn1"));
        m.add(Diagnostic::new(DiagnosticLevel::Error, "err1"));
        assert!(!m.is_empty());
        assert!(m.has_errors());
        assert_eq!(m.len(), 2);
    }

    #[test]
    fn multi_diagnostic_emit_all_only_errors() {
        let mut m = MultiDiagnostic::new();
        m.add(Diagnostic::new(DiagnosticLevel::Warning, "warn1"));
        m.add(Diagnostic::new(DiagnosticLevel::Error, "err1"));
        m.add(Diagnostic::new(DiagnosticLevel::Error, "err2"));
        let tokens = m.emit_all();
        let s = tokens.to_string();
        // 应该包含两个 compile_error
        assert_eq!(s.matches("compile_error").count(), 2);
    }

    #[test]
    fn multi_diagnostic_emit_all_no_errors_is_empty() {
        let mut m = MultiDiagnostic::new();
        m.add(Diagnostic::new(DiagnosticLevel::Warning, "warn1"));
        m.add(Diagnostic::new(DiagnosticLevel::Note, "note1"));
        assert!(m.emit_all().is_empty());
    }

    #[test]
    fn multi_diagnostic_into_result_ok() {
        let mut m = MultiDiagnostic::new();
        m.add(Diagnostic::new(DiagnosticLevel::Warning, "just a warning"));
        assert!(m.into_result().is_ok());
    }

    #[test]
    fn multi_diagnostic_into_result_err() {
        let mut m = MultiDiagnostic::new();
        m.add(Diagnostic::new(DiagnosticLevel::Error, "fatal"));
        let result = m.into_result();
        assert!(result.is_err());
    }

    #[test]
    fn multi_diagnostic_iter() {
        let mut m = MultiDiagnostic::new();
        m.add(Diagnostic::new(DiagnosticLevel::Error, "e1"));
        m.add(Diagnostic::new(DiagnosticLevel::Warning, "w1"));
        let msgs: Vec<&str> = m.iter().map(|d| d.message()).collect();
        assert_eq!(msgs, &["e1", "w1"]);
    }

    // -- 便捷函数 --

    #[test]
    fn convenience_error() {
        let d = error("fail");
        assert_eq!(d.level(), &DiagnosticLevel::Error);
        assert_eq!(d.message(), "fail");
        assert!(d.span().is_none());
    }

    #[test]
    fn convenience_error_at() {
        let d = error_at(Span::call_site(), "fail here");
        assert_eq!(d.level(), &DiagnosticLevel::Error);
        assert!(d.span().is_some());
    }

    #[test]
    fn convenience_warning() {
        let d = warning("careful");
        assert_eq!(d.level(), &DiagnosticLevel::Warning);
        assert_eq!(d.message(), "careful");
    }

    // -- Edge Cases --

    #[test]
    fn diagnostic_empty_message() {
        let d = Diagnostic::new(DiagnosticLevel::Error, "");
        assert_eq!(d.message(), "");
        let tokens = d.to_compile_error();
        assert!(tokens.to_string().contains("compile_error"));
    }

    #[test]
    fn multi_diagnostic_empty_emit() {
        let m = MultiDiagnostic::new();
        assert!(m.emit_all().is_empty());
    }

    #[test]
    fn multi_diagnostic_into_result_empty_is_ok() {
        let m = MultiDiagnostic::new();
        assert!(m.into_result().is_ok());
    }

    #[test]
    fn diagnostic_help_without_note() {
        let d = Diagnostic::new(DiagnosticLevel::Error, "msg").with_help("try this");
        let formatted = d.format_message();
        assert!(formatted.contains("help: try this"));
        assert!(!formatted.contains("note:"));
    }

    #[test]
    fn diagnostic_note_without_help() {
        let d = Diagnostic::new(DiagnosticLevel::Error, "msg").with_note("context");
        let formatted = d.format_message();
        assert!(formatted.contains("note: context"));
        assert!(!formatted.contains("help:"));
    }
}
