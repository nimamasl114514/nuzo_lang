//! # MatchSync 核心展开逻辑
//!
//! 为 `#[derive(MatchSync)]` 提供枚举解析与代码生成。
//!
//! ## 支持的属性
//!
//! ### 枚举级别：`#[match_sync(...)]`
//!
//! | 属性 | 说明 | 示例 |
//! |------|------|------|
//! | `prefix = "..."` | 自定义方法名前缀（默认 `on_`） | `#[match_sync(prefix = "visit_")]` |
//! | `result` | 生成 `Result` 返回版本 | `#[match_sync(result)]` |
//! | `mutable` | 生成可变 handler 版本 | `#[match_sync(mutable)]` |
//!
//! ### 变体级别：`#[match_sync(skip)]`
//!
//! 标注在某变体上，该变体在 trait 中提供默认 panic 实现，不强制 handler 实现。
//!
//! ## 生成的代码结构
//!
//! ```ignore
//! #[derive(MatchSync)]
//! #[match_sync(prefix = "visit_", result, mutable)]
//! enum Expr {
//!     Literal(i64),
//!     #[match_sync(skip)]
//!     Internal,
//! }
//!
//! // 生成：
//! pub trait MatchSyncExpr<R> {
//!     fn visit_literal(&self, field0: &i64) -> R;
//! }
//! pub trait MatchSyncExprMut<R> {
//!     fn visit_literal(&mut self, field0: &i64) -> R;
//! }
//! pub trait TryMatchSyncExpr<R, E> {
//!     fn visit_literal(&self, field0: &i64) -> Result<R, E>;
//! }
//! pub trait TryMatchSyncExprMut<R, E> {
//!     fn visit_literal(&mut self, field0: &i64) -> Result<R, E>;
//! }
//! ```

use crate::diag::SpannedError;
use crate::validate::validate_no_duplicate_attrs;
use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::{Attribute, Data, DeriveInput, Fields, Ident, LitStr};

// ══════════════════════════════════════════════════════════════════
// 配置解析
// ══════════════════════════════════════════════════════════════════

/// 枚举级别配置
#[derive(Debug, Clone)]
struct EnumConfig {
    prefix: String,
    result: bool,
    mutable: bool,
    visibility: String,
}

impl Default for EnumConfig {
    fn default() -> Self {
        Self {
            prefix: "on_".to_string(),
            result: false,
            mutable: false,
            visibility: "pub".to_string(),
        }
    }
}

/// 变体级别配置
#[derive(Debug, Clone, Default)]
struct VariantConfig {
    skip: bool,
}

fn parse_enum_attrs(attrs: &[Attribute]) -> syn::Result<EnumConfig> {
    let mut config = EnumConfig::default();
    for attr in attrs {
        if !attr.path().is_ident("match_sync") {
            continue;
        }
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("prefix") {
                let value: LitStr = meta.value()?.parse()?;
                config.prefix = value.value();
                Ok(())
            } else if meta.path.is_ident("result") {
                config.result = true;
                Ok(())
            } else if meta.path.is_ident("mutable") {
                config.mutable = true;
                Ok(())
            } else if meta.path.is_ident("visibility") {
                let value: LitStr = meta.value()?.parse()?;
                let vis_str = value.value();
                // 验证可见性值合法性
                match vis_str.as_str() {
                    "pub" | "pub(crate)" | "pub(super)" => {}
                    s if s.starts_with("pub(in ") => {}
                    _ => {
                        return Err(SpannedError::new_spanned(
                            &value,
                            format!("invalid visibility `{vis_str}`; expected: pub, pub(crate), pub(super), or pub(in ...)"),
                        ).into_inner());
                    }
                }
                config.visibility = vis_str;
                Ok(())
            } else {
                Err(SpannedError::new_spanned(
                    &meta.path,
                    "unknown match_sync attribute; expected: prefix, result, mutable, visibility",
                ).into_inner())
            }
        })?;
    }
    Ok(config)
}

fn parse_variant_attrs(attrs: &[Attribute]) -> syn::Result<VariantConfig> {
    validate_no_duplicate_attrs(attrs, "match_sync")?;
    let mut config = VariantConfig::default();
    for attr in attrs {
        if !attr.path().is_ident("match_sync") {
            continue;
        }
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("skip") {
                config.skip = true;
                Ok(())
            } else {
                Err(SpannedError::new_spanned(
                    &meta.path,
                    "unknown match_sync variant attribute; expected: skip",
                )
                .into_inner())
            }
        })?;
    }
    Ok(config)
}

// ══════════════════════════════════════════════════════════════════
// 核心展开
// ══════════════════════════════════════════════════════════════════

pub fn expand_match_sync(input: &DeriveInput) -> syn::Result<TokenStream> {
    let enum_name = &input.ident;
    let data_enum = match &input.data {
        Data::Enum(e) => e,
        _ => {
            return Err(SpannedError::new_spanned(
                enum_name,
                "MatchSync can only be derived for enums",
            )
            .into_inner());
        }
    };

    validate_no_duplicate_attrs(&input.attrs, "match_sync")?;
    let enum_config = parse_enum_attrs(&input.attrs)?;

    // 收集变体信息
    let mut variants = Vec::with_capacity(data_enum.variants.len());
    for variant in &data_enum.variants {
        let vconfig = parse_variant_attrs(&variant.attrs)?;
        let method_name =
            format_ident!("{}{}", enum_config.prefix, camel_to_snake(&variant.ident.to_string()));
        variants.push((variant, vconfig, method_name));
    }

    let mut outputs = Vec::new();

    // 不可变版本（始终生成）
    outputs.push(generate_version(&VersionContext {
        enum_name,
        variants: &variants,
        enum_config: &enum_config,
        generics: &input.generics,
        mutable: false,
    })?);

    // 可变版本（按需生成）
    if enum_config.mutable {
        outputs.push(generate_version(&VersionContext {
            enum_name,
            variants: &variants,
            enum_config: &enum_config,
            generics: &input.generics,
            mutable: true,
        })?);
    }

    Ok(quote! { #(#outputs)* })
}

/// `generate_version` 的上下文参数，避免 8 参数函数签名
struct VersionContext<'a> {
    enum_name: &'a Ident,
    variants: &'a [(&'a syn::Variant, VariantConfig, Ident)],
    enum_config: &'a EnumConfig,
    generics: &'a syn::Generics,
    mutable: bool,
}

fn generate_version(ctx: &VersionContext<'_>) -> syn::Result<TokenStream> {
    let enum_name = ctx.enum_name;
    let variants = ctx.variants;
    let enum_config = ctx.enum_config;
    let mutable = ctx.mutable;
    let (impl_generics, type_generics, where_clause) = ctx.generics.split_for_impl();
    let trait_name = trait_name(enum_name, mutable, enum_config.result);

    // 构建 trait generics：原始 + R (+ E)
    let mut trait_generics = ctx.generics.clone();
    trait_generics.params.push(syn::parse_quote!(R));
    if enum_config.result {
        trait_generics.params.push(syn::parse_quote!(E));
    }
    let (_trait_impl_g, trait_type_g, trait_where_g) = trait_generics.split_for_impl();

    // trait 方法
    let trait_methods: Vec<TokenStream> = variants
        .iter()
        .map(|(variant, vconfig, method_name)| {
            generate_trait_method(
                variant,
                vconfig,
                method_name,
                enum_name,
                enum_config.result,
                mutable,
            )
        })
        .collect();

    // match 分支
    let match_arms: Vec<TokenStream> = variants
        .iter()
        .map(|(variant, _vconfig, method_name)| {
            generate_match_arm(variant, method_name, enum_config.result)
        })
        .collect();

    // 方法名与签名
    let (fn_name, handler_ty, match_target, ret_ty) = if mutable {
        if enum_config.result {
            (
                quote!(try_match_sync_mut),
                quote!(&mut impl #trait_name #trait_type_g),
                quote!(&*self),
                quote!(Result<R, E>),
            )
        } else {
            (
                quote!(match_sync_mut),
                quote!(&mut impl #trait_name #trait_type_g),
                quote!(&*self),
                quote!(R),
            )
        }
    } else {
        if enum_config.result {
            (
                quote!(try_match_sync),
                quote!(&impl #trait_name #trait_type_g),
                quote!(self),
                quote!(Result<R, E>),
            )
        } else {
            (quote!(match_sync), quote!(&impl #trait_name #trait_type_g), quote!(self), quote!(R))
        }
    };

    // 方法上的泛型参数
    let fn_generics = if enum_config.result { quote!(<R, E>) } else { quote!(<R>) };

    // self 参数
    let self_param = if mutable { quote!(&mut self) } else { quote!(&self) };

    // 动态可见性
    let vis: proc_macro2::TokenStream =
        enum_config.visibility.parse().expect("visibility should be valid after validation");

    Ok(quote! {
        #vis trait #trait_name #trait_type_g #trait_where_g {
            #(#trait_methods)*
        }

        impl #impl_generics #enum_name #type_generics #where_clause {
            #vis fn #fn_name #fn_generics(#self_param, handler: #handler_ty) -> #ret_ty {
                match #match_target {
                    #(#match_arms)*
                }
            }
        }
    })
}

fn generate_trait_method(
    variant: &syn::Variant,
    vconfig: &VariantConfig,
    method_name: &Ident,
    enum_name: &Ident,
    result: bool,
    mutable: bool,
) -> TokenStream {
    let self_param = if mutable { quote!(&mut self) } else { quote!(&self) };

    let ret_ty = if result { quote!(Result<R, E>) } else { quote!(R) };

    match &variant.fields {
        Fields::Unit => {
            if vconfig.skip {
                let panic_msg = format!(
                    "MatchSync: variant `{}::{}` is skipped and has no handler",
                    enum_name, variant.ident
                );
                quote! {
                    fn #method_name(#self_param) -> #ret_ty {
                        panic!(#panic_msg)
                    }
                }
            } else {
                quote! {
                    fn #method_name(#self_param) -> #ret_ty;
                }
            }
        }
        Fields::Unnamed(fields) => {
            let field_count = fields.unnamed.len();
            let field_idents: Vec<Ident> =
                (0..field_count).map(|i| format_ident!("field{}", i)).collect();
            let field_types: Vec<&syn::Type> = fields.unnamed.iter().map(|f| &f.ty).collect();

            let params = field_idents.iter().zip(field_types.iter()).map(|(id, ty)| {
                let borrowed = borrowed_type(ty);
                quote! { #id: &#borrowed }
            });

            if vconfig.skip {
                let panic_msg = format!(
                    "MatchSync: variant `{}::{}` is skipped and has no handler",
                    enum_name, variant.ident
                );
                quote! {
                    fn #method_name(#self_param, #(#params),*) -> #ret_ty {
                        let _ = (#(#field_idents),*);
                        panic!(#panic_msg)
                    }
                }
            } else {
                quote! {
                    fn #method_name(#self_param, #(#params),*) -> #ret_ty;
                }
            }
        }
        Fields::Named(fields) => {
            let field_idents: Vec<&Ident> =
                fields.named.iter().map(|f| f.ident.as_ref().unwrap()).collect();
            let field_types: Vec<&syn::Type> = fields.named.iter().map(|f| &f.ty).collect();

            let params = field_idents.iter().zip(field_types.iter()).map(|(id, ty)| {
                let borrowed = borrowed_type(ty);
                quote! { #id: &#borrowed }
            });

            if vconfig.skip {
                let panic_msg = format!(
                    "MatchSync: variant `{}::{}` is skipped and has no handler",
                    enum_name, variant.ident
                );
                quote! {
                    fn #method_name(#self_param, #(#params),*) -> #ret_ty {
                        let _ = (#(#field_idents),*);
                        panic!(#panic_msg)
                    }
                }
            } else {
                quote! {
                    fn #method_name(#self_param, #(#params),*) -> #ret_ty;
                }
            }
        }
    }
}

fn generate_match_arm(variant: &syn::Variant, method_name: &Ident, result: bool) -> TokenStream {
    let variant_ident = &variant.ident;

    match &variant.fields {
        Fields::Unit => {
            if result {
                quote!(Self::#variant_ident => handler.#method_name()?,)
            } else {
                quote!(Self::#variant_ident => handler.#method_name(),)
            }
        }
        Fields::Unnamed(_) => {
            let field_count = variant.fields.len();
            let field_idents: Vec<Ident> =
                (0..field_count).map(|i| format_ident!("field{}", i)).collect();
            if result {
                quote! {
                    Self::#variant_ident(#(#field_idents),*) => handler.#method_name(#(#field_idents),*)?,
                }
            } else {
                quote! {
                    Self::#variant_ident(#(#field_idents),*) => handler.#method_name(#(#field_idents),*),
                }
            }
        }
        Fields::Named(_) => {
            let field_idents: Vec<&Ident> =
                variant.fields.iter().map(|f| f.ident.as_ref().unwrap()).collect();
            if result {
                quote! {
                    Self::#variant_ident { #(#field_idents),* } => handler.#method_name(#(#field_idents),*)?,
                }
            } else {
                quote! {
                    Self::#variant_ident { #(#field_idents),* } => handler.#method_name(#(#field_idents),*),
                }
            }
        }
    }
}

fn trait_name(enum_name: &Ident, mutable: bool, result: bool) -> Ident {
    match (mutable, result) {
        (false, false) => format_ident!("MatchSync{}", enum_name),
        (false, true) => format_ident!("TryMatchSync{}", enum_name),
        (true, false) => format_ident!("MatchSync{}Mut", enum_name),
        (true, true) => format_ident!("TryMatchSync{}Mut", enum_name),
    }
}

// ══════════════════════════════════════════════════════════════════
// 工具函数
// ══════════════════════════════════════════════════════════════════

/// 判断类型是否为指定名称的简单路径类型（如 `String`、`Vec`）
fn is_type_name(ty: &syn::Type, name: &str) -> bool {
    if let syn::Type::Path(type_path) = ty {
        type_path.path.segments.len() == 1 && type_path.path.segments[0].ident == name
    } else {
        false
    }
}

/// 提取 `Vec<T>` 中 `T` 的类型
fn extract_vec_inner(ty: &syn::Type) -> Option<&syn::Type> {
    if let syn::Type::Path(type_path) = ty
        && type_path.path.segments.len() == 1
        && type_path.path.segments[0].ident == "Vec"
        && let syn::PathArguments::AngleBracketed(args) = &type_path.path.segments[0].arguments
        && args.args.len() == 1
        && let syn::GenericArgument::Type(inner) = &args.args[0]
    {
        return Some(inner);
    }
    None
}

/// 将类型转换为其借用形式，用于生成 clippy 友好的方法参数：
/// - `String` → `str`（生成 `&str` 而非 `&String`）
/// - `Vec<T>` → `[T]`（生成 `&[T]` 而非 `&Vec<T>`）
/// - 其他类型保持不变
fn borrowed_type(ty: &syn::Type) -> TokenStream {
    if is_type_name(ty, "String") {
        quote! { str }
    } else if let Some(inner) = extract_vec_inner(ty) {
        quote! { [#inner] }
    } else {
        quote! { #ty }
    }
}

fn camel_to_snake(s: &str) -> String {
    let mut result = String::new();
    let chars: Vec<char> = s.chars().collect();
    for i in 0..chars.len() {
        let ch = chars[i];
        if ch.is_uppercase() {
            if i > 0
                && (chars[i - 1].is_lowercase()
                    || (i + 1 < chars.len() && chars[i + 1].is_lowercase()))
            {
                result.push('_');
            }
            result.push(ch.to_ascii_lowercase());
        } else {
            result.push(ch);
        }
    }
    result
}

// ══════════════════════════════════════════════════════════════════
// Tests
// ══════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;
    use syn::parse_quote;

    // ── camel_to_snake ──────────────────────────────────────

    #[test]
    fn camel_to_snake_basic() {
        assert_eq!(camel_to_snake("LoadK"), "load_k");
        assert_eq!(camel_to_snake("ArrayNew"), "array_new");
        assert_eq!(camel_to_snake("GetCaptured"), "get_captured");
    }

    #[test]
    fn camel_to_snake_all_caps() {
        assert_eq!(camel_to_snake("JSON"), "json");
        assert_eq!(camel_to_snake("EOF"), "eof");
        assert_eq!(camel_to_snake("URL"), "url");
    }

    #[test]
    fn camel_to_snake_mixed_caps() {
        assert_eq!(camel_to_snake("GetURL"), "get_url");
        assert_eq!(camel_to_snake("ParseJSON"), "parse_json");
    }

    #[test]
    fn camel_to_snake_single_char() {
        assert_eq!(camel_to_snake("A"), "a");
        assert_eq!(camel_to_snake("X"), "x");
    }

    #[test]
    fn camel_to_snake_already_snake() {
        assert_eq!(camel_to_snake("already_snake"), "already_snake");
    }

    // ── expand_match_sync: 单元变体 ─────────────────────────

    #[test]
    fn unit_variants() {
        let input: DeriveInput = parse_quote! {
            enum Color {
                Red,
                Green,
                Blue,
            }
        };

        let result = expand_match_sync(&input);
        assert!(result.is_ok(), "unit variants should succeed");
        let code = result.unwrap().to_string();

        assert!(code.contains("pub trait MatchSyncColor < R >"), "trait declaration");
        assert!(code.contains("fn on_red (& self) -> R"), "on_red method");
        assert!(code.contains("fn on_green (& self) -> R"), "on_green method");
        assert!(code.contains("fn on_blue (& self) -> R"), "on_blue method");
        assert!(
            code.contains(
                "pub fn match_sync < R > (& self , handler : & impl MatchSyncColor < R >) -> R"
            ),
            "match_sync method"
        );
        assert!(code.contains("Self :: Red => handler . on_red ()"), "Red arm");
        assert!(code.contains("Self :: Green => handler . on_green ()"), "Green arm");
        assert!(code.contains("Self :: Blue => handler . on_blue ()"), "Blue arm");
    }

    // ── expand_match_sync: 元组变体 ─────────────────────────

    #[test]
    fn tuple_variants() {
        let input: DeriveInput = parse_quote! {
            enum Message {
                Write(String),
                Move(i32, i32),
            }
        };

        let result = expand_match_sync(&input);
        assert!(result.is_ok(), "tuple variants should succeed");
        let code = result.unwrap().to_string();

        assert!(code.contains("pub trait MatchSyncMessage < R >"), "trait declaration");
        assert!(code.contains("fn on_write (& self , field0 : & str) -> R"), "on_write method");
        assert!(
            code.contains("fn on_move (& self , field0 : & i32 , field1 : & i32) -> R"),
            "on_move method"
        );
        assert!(
            code.contains("Self :: Write (field0) => handler . on_write (field0)"),
            "Write arm"
        );
        assert!(
            code.contains("Self :: Move (field0 , field1) => handler . on_move (field0 , field1)"),
            "Move arm"
        );
    }

    // ── expand_match_sync: 命名字段变体 ─────────────────────

    #[test]
    fn named_variants() {
        let input: DeriveInput = parse_quote! {
            enum Event {
                Click { x: i32, y: i32 },
                Key { code: u32 },
            }
        };

        let result = expand_match_sync(&input);
        assert!(result.is_ok(), "named variants should succeed");
        let code = result.unwrap().to_string();

        assert!(code.contains("pub trait MatchSyncEvent < R >"), "trait declaration");
        assert!(
            code.contains("fn on_click (& self , x : & i32 , y : & i32) -> R"),
            "on_click method"
        );
        assert!(code.contains("fn on_key (& self , code : & u32) -> R"), "on_key method");
        assert!(
            code.contains("Self :: Click { x , y } => handler . on_click (x , y)"),
            "Click arm"
        );
        assert!(code.contains("Self :: Key { code } => handler . on_key (code)"), "Key arm");
    }

    // ── expand_match_sync: 泛型枚举 ─────────────────────────

    #[test]
    fn generic_enum() {
        let input: DeriveInput = parse_quote! {
            enum MyResult<T, E> {
                Ok(T),
                Err(E),
            }
        };

        let result = expand_match_sync(&input);
        assert!(result.is_ok(), "generic enum should succeed");
        let code = result.unwrap().to_string();

        assert!(code.contains("pub trait MatchSyncMyResult < T , E , R >"), "trait with generics");
        assert!(code.contains("fn on_ok (& self , field0 : & T) -> R"), "on_ok method");
        assert!(code.contains("fn on_err (& self , field0 : & E) -> R"), "on_err method");
        assert!(code.contains("impl < T , E > MyResult < T , E >"), "impl with generics");
        assert!(code.contains("pub fn match_sync < R > (& self , handler : & impl MatchSyncMyResult < T , E , R >) -> R"), "match_sync with generics");
    }

    // ── expand_match_sync: 混合变体 ─────────────────────────

    #[test]
    fn mixed_variants() {
        let input: DeriveInput = parse_quote! {
            enum Expr {
                Literal(i64),
                Add { left: Box<Expr>, right: Box<Expr> },
                Neg(Box<Expr>),
                Nil,
            }
        };

        let result = expand_match_sync(&input);
        assert!(result.is_ok(), "mixed variants should succeed");
        let code = result.unwrap().to_string();

        assert!(code.contains("fn on_literal (& self , field0 : & i64) -> R"), "on_literal");
        assert!(
            code.contains(
                "fn on_add (& self , left : & Box < Expr > , right : & Box < Expr >) -> R"
            ),
            "on_add"
        );
        assert!(code.contains("fn on_neg (& self , field0 : & Box < Expr >) -> R"), "on_neg");
        assert!(code.contains("fn on_nil (& self) -> R"), "on_nil");
        assert!(
            code.contains("Self :: Literal (field0) => handler . on_literal (field0)"),
            "Literal arm"
        );
        assert!(
            code.contains("Self :: Add { left , right } => handler . on_add (left , right)"),
            "Add arm"
        );
        assert!(code.contains("Self :: Neg (field0) => handler . on_neg (field0)"), "Neg arm");
        assert!(code.contains("Self :: Nil => handler . on_nil ()"), "Nil arm");
    }

    // ── expand_match_sync: 非枚举拒绝 ───────────────────────

    #[test]
    fn rejects_struct() {
        let input: DeriveInput = parse_quote! {
            struct Point { x: i32, y: i32 }
        };

        let result = expand_match_sync(&input);
        assert!(result.is_err(), "struct should be rejected");
        let err = result.unwrap_err().to_string();
        assert!(err.contains("only be derived for enums"), "error message should mention enums");
    }

    #[test]
    fn rejects_union() {
        let input: DeriveInput = parse_quote! {
            union MyUnion { a: i32, b: f32 }
        };

        let result = expand_match_sync(&input);
        assert!(result.is_err(), "union should be rejected");
    }

    // ── expand_match_sync: 空枚举 ───────────────────────────

    #[test]
    fn empty_enum() {
        let input: DeriveInput = parse_quote! {
            enum Empty {}
        };

        let result = expand_match_sync(&input);
        assert!(result.is_ok(), "empty enum should succeed");
        let code = result.unwrap().to_string();

        assert!(code.contains("pub trait MatchSyncEmpty < R >"), "trait declaration");
        assert!(
            code.contains(
                "pub fn match_sync < R > (& self , handler : & impl MatchSyncEmpty < R >) -> R"
            ),
            "match_sync method"
        );
    }

    // ── expand_match_sync: 泛型 + where clause ──────────────

    #[test]
    fn generic_with_where_clause() {
        let input: DeriveInput = parse_quote! {
            enum Wrapper<T>
            where
                T: Clone,
            {
                Value(T),
                Empty,
            }
        };

        let result = expand_match_sync(&input);
        assert!(result.is_ok(), "generic with where clause should succeed");
        let code = result.unwrap().to_string();

        assert!(code.contains("pub trait MatchSyncWrapper < T , R >"), "trait with generics");
        assert!(code.contains("where T : Clone"), "where clause preserved");
        assert!(code.contains("impl < T > Wrapper < T >"), "impl with generics");
    }

    // ── 新功能测试: 自定义前缀 ───────────────────────────────

    #[test]
    fn custom_prefix() {
        let input: DeriveInput = parse_quote! {
            #[match_sync(prefix = "visit_")]
            enum Expr {
                Literal(i64),
            }
        };

        let result = expand_match_sync(&input);
        assert!(result.is_ok(), "custom prefix should succeed");
        let code = result.unwrap().to_string();

        assert!(
            code.contains("fn visit_literal (& self , field0 : & i64) -> R"),
            "visit_literal method"
        );
        assert!(code.contains("handler . visit_literal (field0)"), "visit_literal call");
    }

    // ── 新功能测试: skip 变体 ────────────────────────────────

    #[test]
    fn skip_variant() {
        let input: DeriveInput = parse_quote! {
            enum Expr {
                Literal(i64),
                #[match_sync(skip)]
                Internal,
            }
        };

        let result = expand_match_sync(&input);
        assert!(result.is_ok(), "skip variant should succeed");
        let code = result.unwrap().to_string();

        assert!(
            code.contains("fn on_literal (& self , field0 : & i64) -> R ;"),
            "on_literal is abstract"
        );
        assert!(
            code.contains("fn on_internal (& self) -> R { panic ! (\"MatchSync: variant `"),
            "on_internal has default body"
        );
        assert!(
            code.contains("Self :: Internal => handler . on_internal ()"),
            "Internal arm still present"
        );
    }

    // ── 新功能测试: mutable 版本 ────────────────────────────

    #[test]
    fn mutable_version() {
        let input: DeriveInput = parse_quote! {
            #[match_sync(mutable)]
            enum Expr {
                Literal(i64),
            }
        };

        let result = expand_match_sync(&input);
        assert!(result.is_ok(), "mutable version should succeed");
        let code = result.unwrap().to_string();

        assert!(code.contains("pub trait MatchSyncExpr < R >"), "immutable trait still generated");
        assert!(code.contains("pub trait MatchSyncExprMut < R >"), "mutable trait generated");
        assert!(
            code.contains("fn on_literal (& mut self , field0 : & i64) -> R"),
            "mutable on_literal"
        );
        assert!(code.contains("pub fn match_sync_mut < R > (& mut self , handler : & mut impl MatchSyncExprMut < R >) -> R"), "match_sync_mut method");
        assert!(code.contains("match & * self"), "reborrow for mutable");
    }

    // ── 新功能测试: result 版本 ─────────────────────────────

    #[test]
    fn result_version() {
        let input: DeriveInput = parse_quote! {
            #[match_sync(result)]
            enum Expr {
                Literal(i64),
            }
        };

        let result = expand_match_sync(&input);
        assert!(result.is_ok(), "result version should succeed");
        let code = result.unwrap().to_string();

        assert!(code.contains("pub trait TryMatchSyncExpr < R , E >"), "result trait generated");
        assert!(
            code.contains("fn on_literal (& self , field0 : & i64) -> Result < R , E >"),
            "result on_literal"
        );
        assert!(code.contains("pub fn try_match_sync < R , E > (& self , handler : & impl TryMatchSyncExpr < R , E >) -> Result < R , E >"), "try_match_sync method");
        assert!(code.contains("handler . on_literal (field0) ?"), "? operator in arm");
    }

    // ── 新功能测试: mutable + result ────────────────────────

    #[test]
    fn mutable_and_result() {
        let input: DeriveInput = parse_quote! {
            #[match_sync(mutable, result)]
            enum Expr {
                Literal(i64),
            }
        };

        let result = expand_match_sync(&input);
        assert!(result.is_ok(), "mutable+result should succeed");
        let code = result.unwrap().to_string();

        assert!(
            code.contains("pub trait TryMatchSyncExprMut < R , E >"),
            "mutable result trait generated"
        );
        assert!(code.contains("pub fn try_match_sync_mut < R , E > (& mut self , handler : & mut impl TryMatchSyncExprMut < R , E >) -> Result < R , E >"), "try_match_sync_mut method");
    }

    // ── 新功能测试: 重复属性校验 ──────────────────────────────

    #[test]
    fn rejects_duplicate_match_sync_attr() {
        let input: DeriveInput = parse_quote! {
            #[match_sync(prefix = "on_")]
            #[match_sync(result)]
            enum Color {
                Red,
            }
        };

        let result = expand_match_sync(&input);
        assert!(result.is_err(), "duplicate match_sync attrs should be rejected");
        let err = result.unwrap_err().to_string();
        assert!(err.contains("duplicate"), "error should mention 'duplicate'");
        assert!(err.contains("match_sync"), "error should mention 'match_sync'");
    }

    // ── 新功能测试: skip 变体 panic 消息包含枚举名 ────────────

    #[test]
    fn skip_variant_panic_includes_enum_name() {
        let input: DeriveInput = parse_quote! {
            enum Expr {
                Literal(i64),
                #[match_sync(skip)]
                Internal,
            }
        };

        let result = expand_match_sync(&input);
        assert!(result.is_ok());
        let code = result.unwrap().to_string();
        // panic 消息应包含枚举名 "Expr"（字符串字面量内 :: 无空格）
        assert!(code.contains("Expr::Internal"), "skip panic should include enum name 'Expr'");
    }

    // ── 新功能测试: visibility 属性 ───────────────────────────

    #[test]
    fn visibility_pub_crate() {
        let input: DeriveInput = parse_quote! {
            #[match_sync(visibility = "pub(crate)")]
            enum Color {
                Red,
            }
        };

        let result = expand_match_sync(&input);
        assert!(result.is_ok(), "pub(crate) visibility should succeed");
        let code = result.unwrap().to_string();
        assert!(code.contains("pub (crate) trait MatchSyncColor"), "trait should be pub(crate)");
        assert!(code.contains("pub (crate) fn match_sync"), "fn should be pub(crate)");
    }

    #[test]
    fn visibility_default_is_pub() {
        let input: DeriveInput = parse_quote! {
            enum Color {
                Red,
            }
        };

        let result = expand_match_sync(&input);
        assert!(result.is_ok());
        let code = result.unwrap().to_string();
        assert!(code.contains("pub trait MatchSyncColor"), "default trait should be pub");
        assert!(code.contains("pub fn match_sync"), "default fn should be pub");
    }

    #[test]
    fn rejects_invalid_visibility() {
        let input: DeriveInput = parse_quote! {
            #[match_sync(visibility = "private")]
            enum Color {
                Red,
            }
        };

        let result = expand_match_sync(&input);
        assert!(result.is_err(), "invalid visibility should be rejected");
        let err = result.unwrap_err().to_string();
        assert!(err.contains("invalid visibility"), "error should mention 'invalid visibility'");
    }

    // ── 新功能测试: 未知属性错误包含 help 提示 ────────────────

    #[test]
    fn unknown_attr_includes_help() {
        let input: DeriveInput = parse_quote! {
            #[match_sync(unknown_attr)]
            enum Color {
                Red,
            }
        };

        let result = expand_match_sync(&input);
        assert!(result.is_err(), "unknown attr should be rejected");
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("unknown match_sync attribute"),
            "error should mention 'unknown match_sync attribute'"
        );
        assert!(err.contains("visibility"), "help should mention 'visibility'");
    }
}
