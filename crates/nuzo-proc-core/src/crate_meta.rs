//! `#[crate_meta]` 内层属性宏核心展开逻辑
//!
//! ## 用法
//!
//! 在 crate 根（`lib.rs`）顶部添加内层属性：
//!
//! ```ignore
//! #![crate_meta(layer = 4, description = "编译器核心", entry_type = "Compiler")]
//! ```
//!
//! ## 展开
//!
//! ```ignore
//! pub const NUZO_CRATE_META: ::nuzo_proc_core::crate_meta::CrateMeta =
//!     ::nuzo_proc_core::crate_meta::CrateMeta {
//!         layer: 4,
//!         description: "编译器核心",
//!         entry_type: "Compiler",
//!     };
//! ```
//!
//! ## 参数
//!
//! | 参数 | 类型 | 必填 | 说明 |
//! |------|------|------|------|
//! | `layer` | 整数字面量 | 是 | crate 所在层级（0-7，对应 L0 基础设施到 L7 工具链） |
//! | `description` | 字符串字面量 | 是 | crate 用途描述 |
//! | `entry_type` | 字符串字面量 | 是 | 入口类型标识（如 `"Compiler"`, `"VM"`, `"Library"`） |
//!
//! 参数顺序任意。缺少必填参数或类型不匹配时返回 `syn::Error`。
//!
//! ## 设计说明
//!
//! Rust `//!` 文档注释无法插值 `env!()` 宏，因此本宏不试图改写 `//!`，
//! 而是生成独立的 `pub const NUZO_CRATE_META` 常量。
//!
//! - **编译期校验** layer 与 Cargo.toml 拓扑一致性：由 `build.rs` 处理
//! - **同步** DEVELOPMENT.md：由 `sync_development.py` 扫描本属性处理
//! - 本模块仅负责宏展开，不执行上述校验/同步逻辑
//!
//! ## 调用约定
//!
//! `nuzo_proc` 入口将本宏声明为 `#[proc_macro_attribute]`，
//! 把属性参数 `TokenStream` 传入 [`expand_crate_meta`]，
//! 并将返回的常量与原 crate 根 item 拼接输出。

use proc_macro2::TokenStream;
use quote::quote;
use syn::{Token, parse::Parse, parse::ParseStream, punctuated::Punctuated};

use crate::parse_utils::{parse_int_lit, parse_string_lit};

// ══════════════════════════════════════════════════════════════════
// CrateMeta 结构体
// ══════════════════════════════════════════════════════════════════

/// Crate 元数据结构 — 由 `#[crate_meta]` 宏生成的 `NUZO_CRATE_META` 常量类型。
///
/// 字段均为 `Copy` 类型，便于编译期常量求值与跨 crate 静态访问。
///
/// # 消费方
///
/// - `build.rs`：读取 `NUZO_CRATE_META.layer` 校验拓扑一致性
/// - `sync_development.py`：扫描 `#[crate_meta]` 同步 `DEVELOPMENT.md`
/// - 运行时反射/工具链：读取 `description` / `entry_type` 展示 crate 信息
#[derive(Debug, Clone, Copy)]
pub struct CrateMeta {
    /// crate 所在层级（0-7，对应 L0 基础设施到 L7 工具链）。
    ///
    /// 层级语义校验由 `build.rs` 完成。
    pub layer: u8,
    /// crate 用途描述（如 `"编译器核心"`）。
    pub description: &'static str,
    /// 入口类型标识（如 `"Compiler"`, `"VM"`, `"Library"`）。
    pub entry_type: &'static str,
}

// ══════════════════════════════════════════════════════════════════
// 属性参数解析
// ══════════════════════════════════════════════════════════════════

/// `#[crate_meta(...)]` 属性参数解析结果。
///
/// 由 `Parse` trait 从 `layer = 4, description = "...", entry_type = "..."`
/// 解析而来。参数顺序任意，三个字段均为必填。
#[derive(Debug)]
pub struct CrateMetaInput {
    pub layer: u8,
    pub description: String,
    pub entry_type: String,
}

impl Parse for CrateMetaInput {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        // 解析逗号分隔的 `key = value` 列表，顺序任意
        let meta_items = Punctuated::<syn::Meta, Token![,]>::parse_terminated(input)?;

        let mut layer: Option<u8> = None;
        let mut description: Option<String> = None;
        let mut entry_type: Option<String> = None;

        for item in &meta_items {
            match item {
                syn::Meta::NameValue(nv) => {
                    if nv.path.is_ident("layer") {
                        layer = Some(parse_int_lit(&nv.value, "layer", " (expected 0..=7)")?);
                    } else if nv.path.is_ident("description") {
                        description = Some(parse_string_lit(&nv.value)?);
                    } else if nv.path.is_ident("entry_type") {
                        entry_type = Some(parse_string_lit(&nv.value)?);
                    } else {
                        return Err(syn::Error::new_spanned(
                            &nv.path,
                            "unknown `crate_meta` parameter; allowed: layer, description, entry_type",
                        ));
                    }
                }
                _ => {
                    return Err(syn::Error::new_spanned(
                        item,
                        "expected `key = value` format in `#[crate_meta(...)]`",
                    ));
                }
            }
        }

        // 缺失必填参数时给出清晰错误（含完整用法提示）
        let layer = layer.ok_or_else(|| {
            syn::Error::new(
                proc_macro2::Span::call_site(),
                "missing required parameter `layer`; \
                 usage: #[crate_meta(layer = N, description = \"...\", entry_type = \"...\")]",
            )
        })?;
        let description = description.ok_or_else(|| {
            syn::Error::new(
                proc_macro2::Span::call_site(),
                "missing required parameter `description`; \
                 usage: #[crate_meta(layer = N, description = \"...\", entry_type = \"...\")]",
            )
        })?;
        let entry_type = entry_type.ok_or_else(|| {
            syn::Error::new(
                proc_macro2::Span::call_site(),
                "missing required parameter `entry_type`; \
                 usage: #[crate_meta(layer = N, description = \"...\", entry_type = \"...\")]",
            )
        })?;

        Ok(CrateMetaInput { layer, description, entry_type })
    }
}

// ══════════════════════════════════════════════════════════════════
// 宏展开
// ══════════════════════════════════════════════════════════════════

/// 展开 `#[crate_meta]` 内层属性宏。
///
/// # 输入
///
/// 属性参数 `TokenStream`，例如：
/// ```ignore
/// layer = 4, description = "编译器核心", entry_type = "Compiler"
/// ```
///
/// # 输出
///
/// ```ignore
/// pub const NUZO_CRATE_META: ::nuzo_proc_core::crate_meta::CrateMeta =
///     ::nuzo_proc_core::crate_meta::CrateMeta {
///         layer: 4,
///         description: "编译器核心",
///         entry_type: "Compiler",
///     };
/// ```
///
/// # 调用约定
///
/// `nuzo_proc` 入口（`#[proc_macro_attribute]`）负责：
/// 1. 将属性参数 `attr: TokenStream` 转换为 `proc_macro2::TokenStream` 传入本函数
/// 2. 将本函数返回的常量与原 crate 根 `item` 拼接输出
///
/// 本函数仅负责生成常量定义，不处理原 item 透传。
pub fn expand_crate_meta(input: TokenStream) -> syn::Result<TokenStream> {
    let CrateMetaInput { layer, description, entry_type } = syn::parse2(input)?;

    Ok(quote! {
        /// Crate 元数据常量 — 由 `#[crate_meta]` 内层属性宏自动生成。
        ///
        /// 供 `build.rs` 拓扑校验与 `sync_development.py` 同步使用。
        #[allow(dead_code)]
        pub const NUZO_CRATE_META: ::nuzo_proc_core::crate_meta::CrateMeta =
            ::nuzo_proc_core::crate_meta::CrateMeta {
                layer: #layer,
                description: #description,
                entry_type: #entry_type,
            };
    })
}

// ══════════════════════════════════════════════════════════════════
// 单元测试
// ══════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    // ── Parse: 正常路径 ────────────────────────────────────

    #[test]
    fn test_parse_full_input() {
        let input: CrateMetaInput =
            syn::parse_str(r#"layer = 4, description = "编译器核心", entry_type = "Compiler""#)
                .unwrap();
        assert_eq!(input.layer, 4);
        assert_eq!(input.description, "编译器核心");
        assert_eq!(input.entry_type, "Compiler");
    }

    #[test]
    fn test_parse_reordered() {
        // 参数顺序任意：entry_type 在前，layer 在中，description 在后
        let input: CrateMetaInput =
            syn::parse_str(r#"entry_type = "VM", layer = 3, description = "虚拟机""#).unwrap();
        assert_eq!(input.layer, 3);
        assert_eq!(input.description, "虚拟机");
        assert_eq!(input.entry_type, "VM");
    }

    #[test]
    fn test_parse_layer_zero() {
        // L0 基础设施层（边界值）
        let input: CrateMetaInput =
            syn::parse_str(r#"layer = 0, description = "error", entry_type = "Library""#).unwrap();
        assert_eq!(input.layer, 0);
    }

    #[test]
    fn test_parse_layer_seven() {
        // L7 工具链层（边界值）
        let input: CrateMetaInput =
            syn::parse_str(r#"layer = 7, description = "tools", entry_type = "Tool""#).unwrap();
        assert_eq!(input.layer, 7);
    }

    // ── Parse: 缺失参数 ────────────────────────────────────

    #[test]
    fn test_parse_missing_layer() {
        let result: Result<CrateMetaInput, _> =
            syn::parse_str(r#"description = "x", entry_type = "Y""#);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("layer"), "error should mention `layer`: {}", err);
    }

    #[test]
    fn test_parse_missing_description() {
        let result: Result<CrateMetaInput, _> = syn::parse_str(r#"layer = 1, entry_type = "Y""#);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("description"), "error should mention `description`: {}", err);
    }

    #[test]
    fn test_parse_missing_entry_type() {
        let result: Result<CrateMetaInput, _> = syn::parse_str(r#"layer = 1, description = "x""#);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("entry_type"), "error should mention `entry_type`: {}", err);
    }

    #[test]
    fn test_parse_empty() {
        let result: Result<CrateMetaInput, _> = syn::parse_str("");
        assert!(result.is_err());
    }

    // ── Parse: 类型不匹配 ──────────────────────────────────

    #[test]
    fn test_parse_layer_must_be_int() {
        let result: Result<CrateMetaInput, _> =
            syn::parse_str(r#"layer = "four", description = "x", entry_type = "Y""#);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("layer"), "error should mention `layer`: {}", err);
    }

    #[test]
    fn test_parse_description_must_be_str() {
        let result: Result<CrateMetaInput, _> =
            syn::parse_str(r#"layer = 1, description = 42, entry_type = "Y""#);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_entry_type_must_be_str() {
        let result: Result<CrateMetaInput, _> =
            syn::parse_str(r#"layer = 1, description = "x", entry_type = 42"#);
        assert!(result.is_err());
    }

    // ── Parse: 未知参数 ────────────────────────────────────

    #[test]
    fn test_parse_unknown_key() {
        let result: Result<CrateMetaInput, _> =
            syn::parse_str(r#"layer = 1, description = "x", entry_type = "Y", bogus = true"#);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("bogus") || err.contains("unknown"),
            "error should mention unknown key: {}",
            err
        );
    }

    #[test]
    fn test_parse_flag_form_rejected() {
        // flag 形式（无值）不支持
        let result: Result<CrateMetaInput, _> =
            syn::parse_str(r#"layer = 1, description = "x", entry_type = "Y", verbose"#);
        assert!(result.is_err());
    }

    // ── expand_crate_meta ──────────────────────────────────

    #[test]
    fn test_expand_basic() {
        let input: TokenStream =
            quote::quote!(layer = 4, description = "编译器核心", entry_type = "Compiler");
        let output = expand_crate_meta(input).unwrap();
        let s = output.to_string();
        assert!(s.contains("NUZO_CRATE_META"), "output: {}", s);
        assert!(s.contains("CrateMeta"), "output: {}", s);
        assert!(s.contains("layer"), "output: {}", s);
        assert!(s.contains("description"), "output: {}", s);
        assert!(s.contains("entry_type"), "output: {}", s);
    }

    #[test]
    fn test_expand_includes_values() {
        let input: TokenStream =
            quote::quote!(layer = 2, description = "error", entry_type = "Library");
        let output = expand_crate_meta(input).unwrap();
        let s = output.to_string();
        // 验证值被插入（layer=2, description="error", entry_type="Library"）
        assert!(s.contains("2"), "output: {}", s);
        assert!(s.contains("error"), "output: {}", s);
        assert!(s.contains("Library"), "output: {}", s);
    }

    #[test]
    fn test_expand_uses_absolute_path() {
        // 验证使用绝对路径 ::nuzo_proc_core::crate_meta::CrateMeta
        let input: TokenStream = quote::quote!(layer = 1, description = "x", entry_type = "Y");
        let output = expand_crate_meta(input).unwrap();
        let s = output.to_string();
        assert!(s.contains("nuzo_proc_core"), "should use absolute path to nuzo_proc_core: {}", s);
        assert!(s.contains("crate_meta"), "should reference crate_meta module: {}", s);
    }

    #[test]
    fn test_expand_rejects_invalid() {
        // 缺 entry_type
        let input: TokenStream = quote::quote!(layer = 1, description = "x");
        let result = expand_crate_meta(input);
        assert!(result.is_err());
    }

    #[test]
    fn test_expand_rejects_bad_type() {
        // layer 类型错误
        let input: TokenStream = quote::quote!(layer = "bad", description = "x", entry_type = "Y");
        let result = expand_crate_meta(input);
        assert!(result.is_err());
    }

    // ── CrateMeta 结构体 ───────────────────────────────────

    #[test]
    fn test_crate_meta_is_copy() {
        // 编译期验证 CrateMeta 是 Copy（供 build.rs / 工具链按值传递）
        fn assert_copy<T: Copy>() {}
        assert_copy::<CrateMeta>();
    }

    #[test]
    fn test_crate_meta_construct() {
        // 验证可以正常构造（确保字段名/类型与展开代码一致）
        let meta = CrateMeta { layer: 4, description: "编译器核心", entry_type: "Compiler" };
        assert_eq!(meta.layer, 4);
        assert_eq!(meta.description, "编译器核心");
        assert_eq!(meta.entry_type, "Compiler");
    }
}
