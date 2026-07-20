//! `#[derive(Trace)]` — GC HeapObject 自动 trace 派生宏
//!
//! 为枚举自动生成 `trace(&self, visitor: &mut Gc)` 方法实现。
//! 消除手写 match 分支的重复代码，确保新增 HeapObject 变体不会遗漏 trace。
//!
//! ## 支持的属性
//!
//! ### 枚举级别：`#[trace(...)]`
//!
//! | 属性 | 说明 | 默认值 |
//! |------|------|--------|
//! | `visitor` | visitor 参数类型路径 | `Gc` |
//! | `method_name` | 生成的方法名 | `"trace"` |
//! | `self_param` | self 参数形式（`&self` / `&mut self`） | `"&self"` |
//!
//! ### 变体级别：`#[trace(skip)]`
//!
//! 标注该变体无需追踪（如不含堆引用的变体）。
//!
//! ### 字段级别：`#[trace(field = "expr")]`
//!
//! 自定义字段追踪表达式。默认行为：对每个字段调用 `visitor.trace(&self.field)`。
//! 用 `#[trace(skip)]` 跳过该字段。
//!
//! ## 示例
//!
//! ```ignore
//! #[derive(Trace)]
//! #[trace(visitor = "Gc", method_name = "trace")]
//! enum HeapObject {
//!     String(Arc<str>),
//!     Array(Vec<Value>),
//!     Closure { captured: Vec<Value>, code_idx: u32 },
//!     #[trace(skip)]
//!     Int(i64),
//! }
//! ```
//!
//! 生成：
//! ```ignore
//! impl HeapObject {
//!     pub fn trace(&self, visitor: &mut Gc) {
//!         match self {
//!             Self::String(f0) => { visitor.trace_value(*f0); }  // Arc<str> → Value → trace
//!             Self::Array(f0) => { for v in f0 { visitor.trace(v); } }
//!             Self::Closure { captured, .. } => { for v in captured { visitor.trace(v); } }
//!             Self::Int(_) => {} // skip
//!         }
//!     }
//! }
//! ```

use crate::diag::SpannedError;
use crate::validate::validate_no_duplicate_attrs;
use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::{Data, DeriveInput, Fields, Ident};

// ══════════════════════════════════════════════════════════════════
// 配置
// ══════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
struct TraceEnumConfig {
    visitor: String,
    method_name: String,
    self_param: String,
    visibility: String,
}

impl Default for TraceEnumConfig {
    fn default() -> Self {
        Self {
            visitor: "Gc".to_string(),
            method_name: "trace".to_string(),
            self_param: "&self".to_string(),
            visibility: "pub".to_string(),
        }
    }
}

#[derive(Debug, Clone, Default)]
struct TraceVariantConfig {
    skip: bool,
}

#[derive(Debug, Clone)]
struct TraceFieldConfig {
    skip: bool,
    custom_expr: Option<String>, // 自定义追踪表达式，如 `"visitor.trace_all(&self.0)"`
}

fn parse_trace_enum_attrs(attrs: &[syn::Attribute]) -> syn::Result<TraceEnumConfig> {
    let mut config = TraceEnumConfig::default();
    for attr in attrs {
        if !attr.path().is_ident("trace") {
            continue;
        }
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("visitor") {
                let value: LitStr = meta.value()?.parse()?;
                config.visitor = value.value();
                Ok(())
            } else if meta.path.is_ident("method_name") {
                let value: LitStr = meta.value()?.parse()?;
                config.method_name = value.value();
                Ok(())
            } else if meta.path.is_ident("self_param") {
                let value: LitStr = meta.value()?.parse()?;
                config.self_param = value.value();
                Ok(())
            } else if meta.path.is_ident("visibility") {
                let value: LitStr = meta.value()?.parse()?;
                match value.value().as_str() {
                        "pub" | "pub(crate)" | "pub(super)" => {}
                        other if other.starts_with("pub(in ") => {}
                        s => {
                        return Err(SpannedError::new_spanned(
                            &value,
                            format!("invalid visibility `{s}`; expected: pub, pub(crate), pub(super), or pub(in ...)"),
                        ).into_inner());
                    }
                }
                config.visibility = value.value();
                Ok(())
            } else {
                Err(SpannedError::new_spanned(
                    &meta.path,
                    "unknown trace attribute; expected: visitor, method_name, self_param, visibility",
                ).into_inner())
            }
        })?;
    }
    Ok(config)
}

fn parse_trace_variant_attrs(attrs: &[syn::Attribute]) -> syn::Result<TraceVariantConfig> {
    validate_no_duplicate_attrs(attrs, "trace")?;
    let mut config = TraceVariantConfig::default();
    for attr in attrs {
        if !attr.path().is_ident("trace") {
            continue;
        }
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("skip") {
                config.skip = true;
                Ok(())
            } else {
                Err(SpannedError::new_spanned(
                    &meta.path,
                    "unknown trace variant attribute; expected: skip",
                )
                .into_inner())
            }
        })?;
    }
    Ok(config)
}

fn parse_trace_field_attrs(attrs: &[syn::Attribute]) -> syn::Result<TraceFieldConfig> {
    let mut config = TraceFieldConfig { skip: false, custom_expr: None };
    for attr in attrs {
        if !attr.path().is_ident("trace") {
            continue;
        }
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("skip") {
                config.skip = true;
                Ok(())
            } else if meta.path.is_ident("field") {
                let value: LitStr = meta.value()?.parse()?;
                config.custom_expr = Some(value.value());
                Ok(())
            } else {
                Err(SpannedError::new_spanned(
                    &meta.path,
                    "unknown trace field attribute; expected: skip, field",
                )
                .into_inner())
            }
        })?;
    }
    Ok(config)
}

use syn::LitStr;

// ══════════════════════════════════════════════════════════════════
// 核心展开
// ══════════════════════════════════════════════════════════════════

/// 入口：从 DeriveInput 生成 trace 方法 impl。
pub fn expand_trace(input: &DeriveInput) -> syn::Result<TokenStream> {
    let enum_name = &input.ident;
    let data_enum = match &input.data {
        Data::Enum(e) => e,
        _ => {
            return Err(SpannedError::new_spanned(
                enum_name,
                "Trace can only be derived for enums",
            )
            .into_inner());
        }
    };

    validate_no_duplicate_attrs(&input.attrs, "trace")?;
    let config = parse_trace_enum_attrs(&input.attrs)?;

    let (impl_generics, type_generics, where_clause) = input.generics.split_for_impl();
    let visitor_ident = format_ident!("{}", config.visitor);
    let method: Ident = format_ident!("{}", config.method_name);
    let self_param: proc_macro2::TokenStream =
        config.self_param.parse().expect("self_param should be valid");
    let vis: proc_macro2::TokenStream =
        config.visibility.parse().expect("visibility should be valid");

    // 为每个变体生成 match 分支
    let arms: Vec<TokenStream> = data_enum
        .variants
        .iter()
        .map(|variant| {
            let vconfig = parse_trace_variant_attrs(&variant.attrs)?;
            let variant_ident = &variant.ident;

            if vconfig.skip {
                // 整个变体跳过
                return Ok(quote!(Self::#variant_ident(..) => {}));
            }

            match &variant.fields {
                Fields::Unit => Ok(quote!(Self::#variant_ident => {})),
                Fields::Unnamed(fields) => {
                    let field_arms: Vec<TokenStream> = fields
                        .unnamed
                        .iter()
                        .enumerate()
                        .map(|(i, field)| {
                            let fconfig = parse_trace_field_attrs(&field.attrs)?;
                            if fconfig.skip {
                                return Ok(quote!(_)); // 忽略此字段
                            }
                            if let Some(ref expr) = fconfig.custom_expr {
                                let e: TokenStream =
                                    expr.parse().expect("custom expr should be valid");
                                return Ok(quote!({ #e }));
                            }
                            // 默认：visitor.trace(&field)
                            let binding = format_ident!("f{}", i);
                            Ok(quote!(#visitor_ident.trace(#binding);))
                        })
                        .collect::<syn::Result<Vec<_>>>()?;

                    let bindings: Vec<Ident> =
                        (0..fields.unnamed.len()).map(|i| format_ident!("f{}", i)).collect();

                    Ok(quote!(Self::#variant_ident(#(#bindings),*) => { #(#field_arms)* }))
                }
                Fields::Named(fields) => {
                    let field_arms: Vec<TokenStream> = fields
                        .named
                        .iter()
                        .map(|field| {
                            let fconfig = parse_trace_field_attrs(&field.attrs)?;
                            let ident = field.ident.as_ref().unwrap();
                            if fconfig.skip {
                                return Ok(quote!()); // 忽略
                            }
                            if let Some(ref expr) = fconfig.custom_expr {
                                let e: TokenStream =
                                    expr.parse().expect("custom expr should be valid");
                                return Ok(quote!({ #e }));
                            }
                            Ok(quote!(#visitor_ident.trace(#ident);))
                        })
                        .collect::<syn::Result<Vec<_>>>()?;

                    Ok(quote!(Self::#variant_ident { .. } => { #(#field_arms)* }))
                }
            }
        })
        .collect::<syn::Result<Vec<_>>>()?;

    Ok(quote! {
        impl #impl_generics #enum_name #type_generics #where_clause {
            #vis fn #method(#self_param, visitor: &mut #visitor_ident) {
                match self {
                    #(#arms)*
                }
            }
        }
    })
}

// ══════════════════════════════════════════════════════════════════
// Tests
// ══════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;
    use syn::parse_quote;

    #[test]
    fn basic_enum() {
        let input: DeriveInput = parse_quote! {
            enum MyHeapObj {
                String(Arc<str>),
                Int(i64),
            }
        };
        let result = expand_trace(&input);
        assert!(result.is_ok());
        let code = result.unwrap().to_string();
        assert!(code.contains("fn trace"));
        assert!(code.contains("visitor"));
    }

    #[test]
    fn skip_variant() {
        let input: DeriveInput = parse_quote! {
            #[derive(Trace)]
            enum Obj {
                Data(Vec<u8>),
                #[trace(skip)]
                RawInt(i64),
            }
        };
        let result = expand_trace(&input);
        assert!(result.is_ok());
    }

    #[test]
    fn custom_visitor_and_method() {
        let input: DeriveInput = parse_quote! {
            #[trace(visitor = "MyGc", method_name = "walk")]
            enum Obj {
                A(String),
            }
        };
        let result = expand_trace(&input);
        assert!(result.is_ok());
        let code = result.unwrap().to_string();
        assert!(code.contains("fn walk"));
        // proc-macro2 的 TokenStream::to_string() 可能在 `&` 和 `mut` 间插入空格（`& mut`），
        // 用 `mut MyGc` 子串匹配兼容两种格式
        assert!(code.contains("mut MyGc"));
    }

    #[test]
    fn rejects_struct() {
        let input: DeriveInput = parse_quote! {
            struct NotAnEnum { x: i64 }
        };
        assert!(expand_trace(&input).is_err());
    }

    #[test]
    fn named_fields() {
        let input: DeriveInput = parse_quote! {
            enum Obj {
                Closure { captured: Vec<Value>, code_idx: u32 },
            }
        };
        let result = expand_trace(&input);
        assert!(result.is_ok());
    }

    #[test]
    fn visibility_pub_crate() {
        let input: DeriveInput = parse_quote! {
            #[trace(visibility = "pub(crate)")]
            enum Obj { A }
        };
        let result = expand_trace(&input);
        assert!(result.is_ok());
        let code = result.unwrap().to_string();
        assert!(code.contains("pub (crate)"));
    }
}
