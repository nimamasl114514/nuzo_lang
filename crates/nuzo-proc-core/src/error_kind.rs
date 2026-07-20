//! # ErrorKind 核心展开逻辑
//!
//! 为 `#[derive(ErrorKind)]` 提供错误枚举的 Display、Severity、Category 自动生成。
//!
//! ## 支持的属性
//!
//! ### 变体级别：`#[error(...)]`
//!
//! | 属性 | 必需 | 说明 | 示例 |
//! |------|------|------|------|
//! | `category` | 是 | 映射到 `ErrorCategory::XXX` 变体名 | `category = "TypeMismatch"` |
//! | `severity` | 是 | 严重级别：Fatal/Error/Warning/Info | `severity = "Error"` |
//! | `message` | 是 | Display 模板，`{field}` 占位符 | `message = "expected {expected}, got {got}"` |
//!
//! ## 生成的代码结构
//!
//! ```ignore
//! #[derive(ErrorKind)]
//! enum CompileError {
//!     #[error(category = "TypeMismatch", severity = "Error", message = "type mismatch: expected {expected}, got {got}")]
//!     TypeMismatch { expected: String, got: String },
//!     #[error(category = "Syntax", severity = "Fatal", message = "unexpected end of file")]
//!     UnexpectedEof,
//! }
//!
//! // 生成：
//! impl std::fmt::Display for CompileError { ... }
//! impl CompileError {
//!     pub fn default_severity(&self) -> ErrorSeverity { ... }
//!     pub fn default_category(&self) -> ErrorCategory { ... }
//! }
//! ```
//!
//! ## 编译期校验
//!
//! - `{field_name}` 占位符必须对应变体的实际字段
//! - `category`、`severity`、`message` 均为必填
//! - 不允许未知属性字段

use crate::diag::SpannedError;
use crate::validate::validate_no_duplicate_attrs;
use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::{Attribute, Data, DeriveInput, Fields, Ident, LitStr};

// ══════════════════════════════════════════════════════════════════
// 配置解析
// ══════════════════════════════════════════════════════════════════

/// 变体级别 `#[error(...)]` 属性的解析结果。
#[derive(Debug, Clone)]
struct ErrorAttr {
    /// 错误分类，映射到 `ErrorCategory::XXX` 变体名
    category: String,
    /// 严重级别：Fatal / Error / Warning / Info
    severity: String,
    /// Display 模板，支持 `{field_name}` 占位符
    message: String,
}

/// 合法的 severity 值列表
const VALID_SEVERITIES: &[&str] = &["Fatal", "Error", "Warning", "Info"];

/// 解析变体上的 `#[error(...)]` 属性。
///
/// 属性语法：`#[error(category = "...", severity = "...", message = "...")]`
fn parse_error_attr(attrs: &[Attribute]) -> syn::Result<Option<ErrorAttr>> {
    validate_no_duplicate_attrs(attrs, "error")?;

    let mut category: Option<String> = None;
    let mut severity: Option<String> = None;
    let mut message: Option<String> = None;
    let mut has_error_attr = false;

    for attr in attrs {
        if !attr.path().is_ident("error") {
            continue;
        }
        has_error_attr = true;

        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("category") {
                let value: LitStr = meta.value()?.parse()?;
                category = Some(value.value());
                Ok(())
            } else if meta.path.is_ident("severity") {
                let value: LitStr = meta.value()?.parse()?;
                let sev = value.value();
                if !VALID_SEVERITIES.contains(&sev.as_str()) {
                    return Err(SpannedError::new_spanned(
                        &value,
                        format!(
                            "invalid severity `{sev}`; expected one of: {}",
                            VALID_SEVERITIES.join(", ")
                        ),
                    )
                    .into_inner());
                }
                severity = Some(sev);
                Ok(())
            } else if meta.path.is_ident("message") {
                let value: LitStr = meta.value()?.parse()?;
                message = Some(value.value());
                Ok(())
            } else {
                Err(SpannedError::new_spanned(
                    &meta.path,
                    "unknown error attribute; expected: category, severity, message",
                )
                .into_inner())
            }
        })?;
    }

    // 如果没有 error 属性，返回 None（由调用方报告缺失 #[error(...)]）
    if !has_error_attr {
        return Ok(None);
    }

    // 校验必填字段：每个属性都报告具体缺失的名称
    let category = category.ok_or_else(|| {
        SpannedError::new(
            proc_macro2::Span::call_site(),
            "missing required attribute `category` in #[error(...)]",
        )
        .into_inner()
    })?;

    let severity = severity.ok_or_else(|| {
        SpannedError::new(
            proc_macro2::Span::call_site(),
            "missing required attribute `severity` in #[error(...)]",
        )
        .into_inner()
    })?;

    let message = message.ok_or_else(|| {
        SpannedError::new(
            proc_macro2::Span::call_site(),
            "missing required attribute `message` in #[error(...)]",
        )
        .into_inner()
    })?;

    Ok(Some(ErrorAttr { category, severity, message }))
}

// ══════════════════════════════════════════════════════════════════
// 占位符校验
// ══════════════════════════════════════════════════════════════════

/// 从消息模板中提取所有 `{field_name}` 占位符。
///
/// 返回占位符名称列表（不含花括号）。
fn extract_placeholders(message: &str) -> Vec<String> {
    let mut placeholders = Vec::new();
    let mut chars = message.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '{' {
            let mut name = String::new();
            loop {
                match chars.next() {
                    Some('}') => {
                        if !name.is_empty() {
                            placeholders.push(name);
                        }
                        break;
                    }
                    Some(inner) => name.push(inner),
                    None => break,
                }
            }
        }
    }

    placeholders
}

/// 校验消息模板中的占位符是否与变体字段匹配。
///
/// - 对于命名结构体变体：占位符名必须与字段名一致
/// - 对于元组变体：占位符名必须为 `field0`, `field1`, ...
/// - 对于单元变体：不允许有任何占位符
fn validate_placeholders(message: &str, fields: &Fields, variant_ident: &Ident) -> syn::Result<()> {
    let placeholders = extract_placeholders(message);

    match fields {
        Fields::Unit => {
            if !placeholders.is_empty() {
                let bad_placeholders: String =
                    placeholders.iter().map(|p| format!("{{{p}}}")).collect::<Vec<_>>().join(", ");
                return Err(SpannedError::new_spanned(
                variant_ident,
                format!(
                    "unit variant `{}` has no fields, but message contains placeholders: {bad_placeholders}",
                    variant_ident,
                ),
            )
            .into_inner());
            }
        }
        Fields::Named(named) => {
            let field_names: Vec<String> =
                named.named.iter().map(|f| f.ident.as_ref().unwrap().to_string()).collect();

            for placeholder in &placeholders {
                if !field_names.contains(placeholder) {
                    let ph_display = format!("{{{placeholder}}}");
                    return Err(SpannedError::new_spanned(
                        variant_ident,
                        format!(
                            "placeholder `{ph_display}` in message does not match any field of variant `{}`; \
                             available fields: {}",
                            variant_ident,
                            field_names.join(", ")
                        ),
                    )
                    .into_inner());
                }
            }
        }
        Fields::Unnamed(unnamed) => {
            let field_count = unnamed.unnamed.len();
            for placeholder in &placeholders {
                // 元组变体的占位符必须是 field0, field1, ...
                if let Some(idx_str) = placeholder.strip_prefix("field")
                    && let Ok(idx) = idx_str.parse::<usize>()
                    && idx < field_count
                {
                    continue;
                }
                let ph_display = format!("{{{placeholder}}}");
                let expected_placeholders: String = (0..field_count)
                    .map(|i| format!("{{field{i}}}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                return Err(SpannedError::new_spanned(
                    variant_ident,
                    format!(
                        "placeholder `{ph_display}` in message does not match any field of tuple variant `{}`; \
                         expected placeholders: {expected_placeholders}",
                        variant_ident,
                    ),
                )
                .into_inner());
            }
        }
    }

    Ok(())
}

// ══════════════════════════════════════════════════════════════════
// 核心展开
// ══════════════════════════════════════════════════════════════════

/// `#[derive(ErrorKind)]` 的核心展开函数。
///
/// 接受 `&DeriveInput` 引用以便在单元测试中直接调用（无需 proc-macro 上下文）。
pub fn expand_error_kind(input: &DeriveInput) -> syn::Result<TokenStream> {
    let enum_name = &input.ident;
    let data_enum = match &input.data {
        Data::Enum(e) => e,
        _ => {
            return Err(SpannedError::new_spanned(
                enum_name,
                "ErrorKind can only be derived for enums",
            )
            .into_inner());
        }
    };

    let (impl_generics, type_generics, where_clause) = input.generics.split_for_impl();

    let mut variants_info = Vec::with_capacity(data_enum.variants.len());
    for variant in &data_enum.variants {
        let error_attr = parse_error_attr(&variant.attrs)?;

        let attr = match error_attr {
            Some(a) => a,
            None => {
                return Err(SpannedError::new_spanned(
                    &variant.ident,
                    format!(
                        "missing `#[error(...)]` attribute on variant `{}`; \
                         each variant must have exactly one #[error(category = ..., severity = ..., message = ...)] \
                         — required to generate Display impl, default_severity(), and default_category()",
                        variant.ident
                    ),
                )
                .into_inner());
            }
        };

        validate_placeholders(&attr.message, &variant.fields, &variant.ident)?;

        variants_info.push((variant, attr));
    }

    // ── 生成 Display impl ──
    let display_impl = generate_display_impl(
        enum_name,
        &variants_info,
        &impl_generics,
        &type_generics,
        where_clause,
    );

    // ── 生成 default_severity 方法 ──
    let severity_impl = generate_severity_impl(
        enum_name,
        &variants_info,
        &impl_generics,
        &type_generics,
        where_clause,
    );

    // ── 生成 default_category 方法 ──
    let category_impl = generate_category_impl(
        enum_name,
        &variants_info,
        &impl_generics,
        &type_generics,
        where_clause,
    );

    Ok(quote! {
        #display_impl
        #severity_impl
        #category_impl
    })
}

// ══════════════════════════════════════════════════════════════════
// Display 实现
// ══════════════════════════════════════════════════════════════════

/// 生成 `impl std::fmt::Display for TheEnum` 代码。
///
/// 对于每个变体，将 message 模板中的 `{field}` 替换为实际字段值的 Display 输出。
fn generate_display_impl(
    enum_name: &Ident,
    variants_info: &[(&syn::Variant, ErrorAttr)],
    impl_generics: &syn::ImplGenerics,
    type_generics: &syn::TypeGenerics,
    where_clause: Option<&syn::WhereClause>,
) -> TokenStream {
    let arms: Vec<TokenStream> = variants_info
        .iter()
        .map(|(variant, attr)| {
            let variant_ident = &variant.ident;
            let message = &attr.message;

            match &variant.fields {
                Fields::Unit => {
                    // 单元变体：消息无占位符，直接写入
                    quote! {
                        Self::#variant_ident => write!(f, #message),
                    }
                }
                Fields::Named(_) => {
                    // 命名字段变体：提取字段名，生成 write! 调用
                    let field_idents: Vec<&Ident> =
                        variant.fields.iter().map(|f| f.ident.as_ref().unwrap()).collect();

                    quote! {
                        Self::#variant_ident { #(#field_idents),* } => {
                            write!(f, #message #(, #field_idents = #field_idents)*)
                        }
                    }
                }
                Fields::Unnamed(_) => {
                    // 元组变体：使用 field0, field1, ... 作为占位符名和绑定名
                    let field_count = variant.fields.len();
                    let field_idents: Vec<Ident> =
                        (0..field_count).map(|i| format_ident!("field{}", i)).collect();

                    quote! {
                        Self::#variant_ident(#(#field_idents),*) => {
                            write!(f, #message #(, #field_idents = #field_idents)*)
                        }
                    }
                }
            }
        })
        .collect();

    quote! {
        impl #impl_generics ::std::fmt::Display for #enum_name #type_generics #where_clause {
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                match self {
                    #(#arms)*
                }
            }
        }
    }
}

// ══════════════════════════════════════════════════════════════════
// Severity 实现
// ══════════════════════════════════════════════════════════════════

/// 生成 `impl TheEnum { pub fn default_severity(&self) -> ErrorSeverity }` 方法。
///
/// 每个变体返回属性中声明的 severity 对应的枚举变体。
fn generate_severity_impl(
    enum_name: &Ident,
    variants_info: &[(&syn::Variant, ErrorAttr)],
    impl_generics: &syn::ImplGenerics,
    type_generics: &syn::TypeGenerics,
    where_clause: Option<&syn::WhereClause>,
) -> TokenStream {
    let arms: Vec<TokenStream> = variants_info
        .iter()
        .map(|(variant, attr)| {
            let variant_ident = &variant.ident;
            let severity_ident = format_ident!("{}", attr.severity);

            match &variant.fields {
                Fields::Unit => {
                    quote! {
                        Self::#variant_ident => ErrorSeverity::#severity_ident,
                    }
                }
                Fields::Named(_) => {
                    quote! {
                        Self::#variant_ident { .. } => ErrorSeverity::#severity_ident,
                    }
                }
                Fields::Unnamed(_) => {
                    quote! {
                        Self::#variant_ident(..) => ErrorSeverity::#severity_ident,
                    }
                }
            }
        })
        .collect();

    quote! {
        impl #impl_generics #enum_name #type_generics #where_clause {
            /// 返回该错误变体的默认严重级别。
            pub fn default_severity(&self) -> ErrorSeverity {
                match self {
                    #(#arms)*
                }
            }
        }
    }
}

// ══════════════════════════════════════════════════════════════════
// Category 实现
// ══════════════════════════════════════════════════════════════════

/// 生成 `impl TheEnum { pub fn default_category(&self) -> ErrorCategory }` 方法。
///
/// 每个变体返回属性中声明的 category 对应的枚举变体。
fn generate_category_impl(
    enum_name: &Ident,
    variants_info: &[(&syn::Variant, ErrorAttr)],
    impl_generics: &syn::ImplGenerics,
    type_generics: &syn::TypeGenerics,
    where_clause: Option<&syn::WhereClause>,
) -> TokenStream {
    let arms: Vec<TokenStream> = variants_info
        .iter()
        .map(|(variant, attr)| {
            let variant_ident = &variant.ident;
            let category_ident = format_ident!("{}", attr.category);

            match &variant.fields {
                Fields::Unit => {
                    quote! {
                        Self::#variant_ident => ErrorCategory::#category_ident,
                    }
                }
                Fields::Named(_) => {
                    quote! {
                        Self::#variant_ident { .. } => ErrorCategory::#category_ident,
                    }
                }
                Fields::Unnamed(_) => {
                    quote! {
                        Self::#variant_ident(..) => ErrorCategory::#category_ident,
                    }
                }
            }
        })
        .collect();

    quote! {
        impl #impl_generics #enum_name #type_generics #where_clause {
            /// 返回该错误变体的默认分类。
            pub fn default_category(&self) -> ErrorCategory {
                match self {
                    #(#arms)*
                }
            }
        }
    }
}

// ══════════════════════════════════════════════════════════════════
// Tests
// ══════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;
    use syn::parse_quote;

    // ── extract_placeholders ──────────────────────────────────

    #[test]
    fn extract_placeholders_basic() {
        let result = extract_placeholders("expected {expected}, got {got}");
        assert_eq!(result, vec!["expected", "got"]);
    }

    #[test]
    fn extract_placeholders_no_placeholders() {
        let result = extract_placeholders("unexpected end of file");
        assert!(result.is_empty());
    }

    #[test]
    fn extract_placeholders_single() {
        let result = extract_placeholders("undefined variable {name}");
        assert_eq!(result, vec!["name"]);
    }

    #[test]
    fn extract_placeholders_empty_braces() {
        // 空花括号 {} 不应产生占位符
        let result = extract_placeholders("hello {} world");
        assert!(result.is_empty());
    }

    #[test]
    fn extract_placeholders_adjacent() {
        let result = extract_placeholders("{a}{b}");
        assert_eq!(result, vec!["a", "b"]);
    }

    // ── expand_error_kind: 命名字段变体 (Happy Path) ────────

    #[test]
    fn named_variant_happy_path() {
        let input: DeriveInput = parse_quote! {
            enum CompileError {
                #[error(category = "TypeMismatch", severity = "Error", message = "type mismatch: expected {expected}, got {got}")]
                TypeMismatch { expected: String, got: String },
            }
        };

        let result = expand_error_kind(&input);
        assert!(result.is_ok(), "named variant should succeed");
        let code = result.unwrap().to_string();

        // Display impl
        assert!(code.contains("impl :: std :: fmt :: Display for CompileError"), "Display impl");
        assert!(code.contains("write ! (f , \"type mismatch: expected {expected}, got {got}\" , expected = expected , got = got)"), "Display write with named fields");

        // Severity impl
        assert!(code.contains("fn default_severity"), "severity method");
        assert!(code.contains("ErrorSeverity :: Error"), "severity variant");

        // Category impl
        assert!(code.contains("fn default_category"), "category method");
        assert!(code.contains("ErrorCategory :: TypeMismatch"), "category variant");
    }

    // ── expand_error_kind: 单元变体 (Happy Path) ────────────

    #[test]
    fn unit_variant_happy_path() {
        let input: DeriveInput = parse_quote! {
            enum CompileError {
                #[error(category = "Syntax", severity = "Fatal", message = "unexpected end of file")]
                UnexpectedEof,
            }
        };

        let result = expand_error_kind(&input);
        assert!(result.is_ok(), "unit variant should succeed");
        let code = result.unwrap().to_string();

        // Display impl
        assert!(
            code.contains("write ! (f , \"unexpected end of file\")"),
            "Display write for unit variant"
        );

        // Severity
        assert!(code.contains("ErrorSeverity :: Fatal"), "Fatal severity");

        // Category
        assert!(code.contains("ErrorCategory :: Syntax"), "Syntax category");
    }

    // ── expand_error_kind: 元组变体 (Happy Path) ────────────

    #[test]
    fn tuple_variant_happy_path() {
        let input: DeriveInput = parse_quote! {
            enum CompileError {
                #[error(category = "Runtime", severity = "Warning", message = "division by zero at line {field0}")]
                DivisionByZero(usize),
            }
        };

        let result = expand_error_kind(&input);
        assert!(result.is_ok(), "tuple variant should succeed");
        let code = result.unwrap().to_string();

        // Display impl
        assert!(
            code.contains("write ! (f , \"division by zero at line {field0}\" , field0 = field0)"),
            "Display write for tuple variant"
        );

        // Severity
        assert!(code.contains("ErrorSeverity :: Warning"), "Warning severity");

        // Category
        assert!(code.contains("ErrorCategory :: Runtime"), "Runtime category");
    }

    // ── expand_error_kind: 多变体 (Happy Path) ──────────────

    #[test]
    fn multiple_variants() {
        let input: DeriveInput = parse_quote! {
            enum CompileError {
                #[error(category = "TypeMismatch", severity = "Error", message = "expected {expected}, got {got}")]
                TypeMismatch { expected: String, got: String },
                #[error(category = "Syntax", severity = "Fatal", message = "unexpected end of file")]
                UnexpectedEof,
                #[error(category = "Runtime", severity = "Warning", message = "unused variable {name}")]
                UnusedVar { name: String },
            }
        };

        let result = expand_error_kind(&input);
        assert!(result.is_ok(), "multiple variants should succeed");
        let code = result.unwrap().to_string();

        // 所有 severity
        assert!(code.contains("ErrorSeverity :: Error"), "Error severity");
        assert!(code.contains("ErrorSeverity :: Fatal"), "Fatal severity");
        assert!(code.contains("ErrorSeverity :: Warning"), "Warning severity");

        // 所有 category
        assert!(code.contains("ErrorCategory :: TypeMismatch"), "TypeMismatch category");
        assert!(code.contains("ErrorCategory :: Syntax"), "Syntax category");
        assert!(code.contains("ErrorCategory :: Runtime"), "Runtime category");
    }

    // ── expand_error_kind: Info severity (Happy Path) ────────

    #[test]
    fn info_severity() {
        let input: DeriveInput = parse_quote! {
            enum CompileError {
                #[error(category = "Info", severity = "Info", message = "compilation note")]
                Note,
            }
        };

        let result = expand_error_kind(&input);
        assert!(result.is_ok());
        let code = result.unwrap().to_string();
        assert!(code.contains("ErrorSeverity :: Info"), "Info severity");
    }

    // ── 编译期校验: 占位符引用不存在的字段 ────────────────────

    #[test]
    fn placeholder_references_nonexistent_field() {
        let input: DeriveInput = parse_quote! {
            enum CompileError {
                #[error(category = "TypeMismatch", severity = "Error", message = "expected {nonexistent}")]
                TypeMismatch { expected: String },
            }
        };

        let result = expand_error_kind(&input);
        assert!(result.is_err(), "nonexistent field placeholder should error");
        let err = result.unwrap_err().to_string();
        assert!(err.contains("nonexistent"), "error should mention the bad placeholder");
        assert!(err.contains("does not match"), "error should explain the mismatch");
    }

    // ── 编译期校验: 单元变体包含占位符 ────────────────────────

    #[test]
    fn unit_variant_with_placeholder() {
        let input: DeriveInput = parse_quote! {
            enum CompileError {
                #[error(category = "Syntax", severity = "Error", message = "error at {line}")]
                UnexpectedEof,
            }
        };

        let result = expand_error_kind(&input);
        assert!(result.is_err(), "unit variant with placeholder should error");
        let err = result.unwrap_err().to_string();
        assert!(err.contains("has no fields"), "error should mention no fields");
        assert!(err.contains("{line}"), "error should show the placeholder");
    }

    // ── 编译期校验: 元组变体占位符名称错误 ────────────────────

    #[test]
    fn tuple_variant_wrong_placeholder_name() {
        let input: DeriveInput = parse_quote! {
            enum CompileError {
                #[error(category = "Runtime", severity = "Error", message = "error at {line}")]
                DivisionByZero(usize),
            }
        };

        let result = expand_error_kind(&input);
        assert!(result.is_err(), "wrong placeholder name should error");
        let err = result.unwrap_err().to_string();
        assert!(err.contains("{line}"), "error should show the bad placeholder");
        assert!(err.contains("field0"), "error should suggest field0");
    }

    // ── 编译期校验: 缺少必填属性 ─────────────────────────────

    #[test]
    fn missing_category_attr() {
        let input: DeriveInput = parse_quote! {
            enum CompileError {
                #[error(severity = "Error", message = "oops")]
                Bad,
            }
        };

        let result = expand_error_kind(&input);
        assert!(result.is_err(), "missing category should error");
        let err = result.unwrap_err().to_string();
        // 修复后：parse_error_attr 检测到 #[error] 存在但 category 缺失，
        // 直接报告具体缺失的属性名，而非回退到 "missing #[error(...)]"
        assert!(err.contains("category"), "error should mention category");
    }

    #[test]
    fn missing_severity_attr() {
        let input: DeriveInput = parse_quote! {
            enum CompileError {
                #[error(category = "TypeMismatch", message = "oops")]
                Bad { expected: String },
            }
        };

        let result = expand_error_kind(&input);
        assert!(result.is_err(), "missing severity should error");
        let err = result.unwrap_err().to_string();
        assert!(err.contains("severity"), "error should mention severity");
    }

    #[test]
    fn missing_message_attr() {
        let input: DeriveInput = parse_quote! {
            enum CompileError {
                #[error(category = "TypeMismatch", severity = "Error")]
                Bad { expected: String },
            }
        };

        let result = expand_error_kind(&input);
        assert!(result.is_err(), "missing message should error");
        let err = result.unwrap_err().to_string();
        assert!(err.contains("message"), "error should mention message");
    }

    // ── 编译期校验: 完全缺少 #[error(...)] 属性 ──────────────

    #[test]
    fn missing_error_attr_entirely() {
        let input: DeriveInput = parse_quote! {
            enum CompileError {
                Bad,
            }
        };

        let result = expand_error_kind(&input);
        assert!(result.is_err(), "missing #[error] attr should error");
        let err = result.unwrap_err().to_string();
        assert!(err.contains("#[error"), "error should mention #[error]");
    }

    // ── 编译期校验: 未知属性字段 ─────────────────────────────

    #[test]
    fn unknown_attr_field() {
        let input: DeriveInput = parse_quote! {
            enum CompileError {
                #[error(category = "TypeMismatch", severity = "Error", message = "oops", foobar = "bad")]
                Bad { expected: String },
            }
        };

        let result = expand_error_kind(&input);
        assert!(result.is_err(), "unknown attr field should error");
        let err = result.unwrap_err().to_string();
        assert!(err.contains("unknown error attribute"), "error should mention unknown attribute");
    }

    // ── 编译期校验: 无效的 severity 值 ──────────────────────

    #[test]
    fn invalid_severity_value() {
        let input: DeriveInput = parse_quote! {
            enum CompileError {
                #[error(category = "TypeMismatch", severity = "Critical", message = "oops")]
                Bad { expected: String },
            }
        };

        let result = expand_error_kind(&input);
        assert!(result.is_err(), "invalid severity should error");
        let err = result.unwrap_err().to_string();
        assert!(err.contains("invalid severity"), "error should mention invalid severity");
        assert!(err.contains("Critical"), "error should show the bad value");
    }

    // ── 编译期校验: 重复的 error 属性 ────────────────────────

    #[test]
    fn duplicate_error_attr() {
        let input: DeriveInput = parse_quote! {
            enum CompileError {
                #[error(category = "A", severity = "Error", message = "x")]
                #[error(category = "B", severity = "Error", message = "y")]
                Bad,
            }
        };

        let result = expand_error_kind(&input);
        assert!(result.is_err(), "duplicate error attr should error");
        let err = result.unwrap_err().to_string();
        assert!(err.contains("duplicate"), "error should mention duplicate");
    }

    // ── 编译期校验: 非枚举拒绝 ──────────────────────────────

    #[test]
    fn rejects_struct() {
        let input: DeriveInput = parse_quote! {
            struct Point { x: i32, y: i32 }
        };

        let result = expand_error_kind(&input);
        assert!(result.is_err(), "struct should be rejected");
        let err = result.unwrap_err().to_string();
        assert!(err.contains("only be derived for enums"), "error should mention enums");
    }

    // ── 泛型枚举 ────────────────────────────────────────────

    #[test]
    fn generic_enum() {
        let input: DeriveInput = parse_quote! {
            enum MyError<T> {
                #[error(category = "Generic", severity = "Error", message = "generic error")]
                Generic(T),
            }
        };

        let result = expand_error_kind(&input);
        assert!(result.is_ok(), "generic enum should succeed");
        let code = result.unwrap().to_string();
        assert!(code.contains("impl < T >"), "impl with generics");
        assert!(code.contains("MyError < T >"), "type with generics");
    }

    // ── 元组变体多字段 ──────────────────────────────────────

    #[test]
    fn tuple_variant_multiple_fields() {
        let input: DeriveInput = parse_quote! {
            enum CompileError {
                #[error(category = "Runtime", severity = "Error", message = "error at {field0}:{field1}")]
                Position(usize, usize),
            }
        };

        let result = expand_error_kind(&input);
        assert!(result.is_ok(), "multi-field tuple variant should succeed");
        let code = result.unwrap().to_string();
        assert!(code.contains("field0 = field0"), "field0 placeholder");
        assert!(code.contains("field1 = field1"), "field1 placeholder");
    }

    // ── Display 模板中含花括号转义 ──────────────────────────

    #[test]
    fn message_with_literal_braces() {
        // Rust 的 write! 宏中 {{ 和 }} 用于转义字面量花括号
        // 这里测试消息中不包含占位符的简单场景
        let input: DeriveInput = parse_quote! {
            enum CompileError {
                #[error(category = "Syntax", severity = "Error", message = "unexpected token")]
                UnexpectedToken,
            }
        };

        let result = expand_error_kind(&input);
        assert!(result.is_ok());
        let code = result.unwrap().to_string();
        assert!(code.contains("unexpected token"), "message preserved");
    }

    // ── 回归测试: 缺失属性报告具体名称 (#7) ──────────────────

    #[test]
    fn test_missing_required_attribute_reports_name() {
        // category 缺失：错误信息应包含 "category"
        let input: DeriveInput = parse_quote! {
            enum CompileError {
                #[error(severity = "Error", message = "oops")]
                Bad,
            }
        };
        let result = expand_error_kind(&input);
        assert!(result.is_err());
        assert!(
            result.unwrap_err().to_string().contains("category"),
            "missing category should report the attribute name"
        );

        // severity 缺失：错误信息应包含 "severity"
        let input: DeriveInput = parse_quote! {
            enum CompileError {
                #[error(category = "TypeMismatch", message = "oops")]
                Bad { expected: String },
            }
        };
        let result = expand_error_kind(&input);
        assert!(result.is_err());
        assert!(
            result.unwrap_err().to_string().contains("severity"),
            "missing severity should report the attribute name"
        );

        // message 缺失：错误信息应包含 "message"
        let input: DeriveInput = parse_quote! {
            enum CompileError {
                #[error(category = "TypeMismatch", severity = "Error")]
                Bad { expected: String },
            }
        };
        let result = expand_error_kind(&input);
        assert!(result.is_err());
        assert!(
            result.unwrap_err().to_string().contains("message"),
            "missing message should report the attribute name"
        );
    }

    // ── 回归测试: 缺失 #[error(...)] 报告原因 (#8) ──────────

    #[test]
    fn test_missing_error_attribute_reports_reason() {
        let input: DeriveInput = parse_quote! {
            enum CompileError {
                Bad,
            }
        };

        let result = expand_error_kind(&input);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        // 应说明为什么需要 #[error(...)]：用于生成 Display / severity / category
        assert!(err.contains("#[error"), "error should mention #[error]");
        assert!(
            err.contains("required to generate"),
            "error should explain why #[error(...)] is required (to generate Display/severity/category)"
        );
    }
}
