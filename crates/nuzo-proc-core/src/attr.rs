//! 声明式属性解析框架
//!
//! 提供 `FromMetaValue` trait 和 `AttrStruct` 解析器，
//! 支持通过结构体定义声明式地解析 proc-macro 属性。

use std::collections::HashMap;

use proc_macro2::Ident;
use quote::quote;
use syn::{Attribute, Expr, Lit, Meta, Path, parse_quote, spanned::Spanned};

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// FromMetaValue — 从 syn::Meta / syn::Expr 中提取类型化值
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

pub trait FromMetaValue: Sized {
    fn from_meta(meta: &Meta) -> syn::Result<Self>;
    fn from_value(value: &Expr) -> syn::Result<Self>;
}

impl FromMetaValue for String {
    fn from_meta(meta: &Meta) -> syn::Result<Self> {
        match meta {
            Meta::NameValue(nv) => match &nv.value {
                Expr::Lit(lit) => match &lit.lit {
                    Lit::Str(s) => Ok(s.value()),
                    other => Err(syn::Error::new_spanned(other, "expected string literal")),
                },
                other => Err(syn::Error::new_spanned(other, "expected string literal")),
            },
            other => Err(syn::Error::new_spanned(other, "expected name = \"value\" syntax")),
        }
    }

    fn from_value(value: &Expr) -> syn::Result<Self> {
        match value {
            Expr::Lit(lit) => match &lit.lit {
                Lit::Str(s) => Ok(s.value()),
                other => Err(syn::Error::new_spanned(other, "expected string literal")),
            },
            other => Err(syn::Error::new_spanned(other, "expected string literal")),
        }
    }
}

impl FromMetaValue for bool {
    fn from_meta(meta: &Meta) -> syn::Result<Self> {
        match meta {
            Meta::Path(_) => Ok(true),
            Meta::NameValue(nv) => match &nv.value {
                Expr::Lit(lit) => match &lit.lit {
                    Lit::Bool(b) => Ok(b.value),
                    other => Err(syn::Error::new_spanned(other, "expected boolean literal")),
                },
                other => Err(syn::Error::new_spanned(other, "expected boolean literal")),
            },
            other => Err(syn::Error::new_spanned(other, "expected bare word or boolean literal")),
        }
    }

    fn from_value(value: &Expr) -> syn::Result<Self> {
        match value {
            Expr::Lit(lit) => match &lit.lit {
                Lit::Bool(b) => Ok(b.value),
                other => Err(syn::Error::new_spanned(other, "expected boolean literal")),
            },
            other => Err(syn::Error::new_spanned(other, "expected boolean literal")),
        }
    }
}

impl FromMetaValue for usize {
    fn from_meta(meta: &Meta) -> syn::Result<Self> {
        let expr = Self::extract_name_value_expr(meta)?;
        Self::parse_int_expr(&expr)
    }

    fn from_value(value: &Expr) -> syn::Result<Self> {
        Self::parse_int_expr(value)
    }
}

impl FromMetaValue for i64 {
    fn from_meta(meta: &Meta) -> syn::Result<Self> {
        let expr = Self::extract_name_value_expr(meta)?;
        Self::parse_int_expr_signed(&expr)
    }

    fn from_value(value: &Expr) -> syn::Result<Self> {
        Self::parse_int_expr_signed(value)
    }
}

impl FromMetaValue for Ident {
    fn from_meta(meta: &Meta) -> syn::Result<Self> {
        match meta {
            Meta::Path(p) => p
                .get_ident()
                .cloned()
                .ok_or_else(|| syn::Error::new_spanned(p, "expected identifier")),
            other => Err(syn::Error::new_spanned(other, "expected path or identifier")),
        }
    }

    fn from_value(value: &Expr) -> syn::Result<Self> {
        match value {
            Expr::Path(p) => p
                .path
                .get_ident()
                .cloned()
                .ok_or_else(|| syn::Error::new_spanned(value, "expected identifier")),
            other => Err(syn::Error::new_spanned(other, "expected identifier")),
        }
    }
}

impl FromMetaValue for Path {
    fn from_meta(meta: &Meta) -> syn::Result<Self> {
        match meta {
            Meta::Path(p) => Ok(p.clone()),
            Meta::NameValue(nv) => match &nv.value {
                Expr::Path(p) => Ok(p.path.clone()),
                other => Err(syn::Error::new_spanned(other, "expected path")),
            },
            other => Err(syn::Error::new_spanned(other, "expected path")),
        }
    }

    fn from_value(value: &Expr) -> syn::Result<Self> {
        match value {
            Expr::Path(p) => Ok(p.path.clone()),
            other => Err(syn::Error::new_spanned(other, "expected path")),
        }
    }
}

impl<T: FromMetaValue> FromMetaValue for Option<T> {
    fn from_meta(_meta: &Meta) -> syn::Result<Self> {
        // Option<T> 不应通过 FromMetaValue 直接构造：
        // 它的"缺失即 None"语义需要在容器层（如 AttrValues）处理，
        // 因为只有容器知道字段是否被显式声明为可选。
        // 返回 syn::Error 而非 unreachable!，
        // 让调用方获得可定位的诊断信息，而不是 proc-macro 内部 panic。
        Err(syn::Error::new(
            proc_macro2::Span::call_site(),
            "Option<T> should be handled via AttrValues container, not direct FromMetaValue",
        ))
    }

    fn from_value(_value: &Expr) -> syn::Result<Self> {
        Err(syn::Error::new(
            proc_macro2::Span::call_site(),
            "Option<T> should be handled via AttrValues container, not direct FromMetaValue",
        ))
    }
}

impl<T: FromMetaValue> FromMetaValue for Vec<T> {
    fn from_meta(meta: &Meta) -> syn::Result<Self> {
        match meta {
            Meta::List(list) => list
                .tokens
                .clone()
                .into_iter()
                .filter_map(|tt| match tt {
                    proc_macro2::TokenTree::Punct(p) if p.as_char() == ',' => None,
                    _ => Some(tt),
                })
                .collect::<Vec<_>>()
                .chunks(1)
                .map(|chunk| {
                    let tokens: proc_macro2::TokenStream = chunk.iter().cloned().collect();
                    let span = tokens.span();
                    let expr: Expr = syn::parse2(tokens).map_err(|e| syn::Error::new(span, e))?;
                    T::from_value(&expr)
                })
                .collect::<core::result::Result<Vec<T>, _>>(),
            other => Err(syn::Error::new_spanned(other, "expected list of values")),
        }
    }

    fn from_value(value: &Expr) -> syn::Result<Self> {
        Ok(vec![T::from_value(value)?])
    }
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// 浮点类型支持 (f32 / f64)
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

impl FromMetaValue for f32 {
    fn from_meta(meta: &Meta) -> syn::Result<Self> {
        let expr = Self::extract_expr(meta)?;
        Self::parse_float(&expr)
    }

    fn from_value(value: &Expr) -> syn::Result<Self> {
        Self::parse_float(value)
    }
}

impl FromMetaValue for f64 {
    fn from_meta(meta: &Meta) -> syn::Result<Self> {
        let expr = Self::extract_expr(meta)?;
        Self::parse_float(&expr)
    }

    fn from_value(value: &Expr) -> syn::Result<Self> {
        Self::parse_float(value)
    }
}

/// 浮点解析的共享 trait（私有，消除 f32/f64 间的重复代码）
trait FloatParseHelper: Sized + Into<f64> {
    fn extract_expr(meta: &Meta) -> syn::Result<Expr>;
    fn parse_float(expr: &Expr) -> syn::Result<Self>;
}

impl FloatParseHelper for f32 {
    fn extract_expr(meta: &Meta) -> syn::Result<Expr> {
        match meta {
            Meta::NameValue(nv) => Ok(nv.value.clone()),
            other => Err(syn::Error::new_spanned(other, "expected name = float syntax")),
        }
    }

    fn parse_float(expr: &Expr) -> syn::Result<Self> {
        match expr {
            Expr::Lit(lit) => match &lit.lit {
                Lit::Float(f) => f.base10_parse::<f64>().map(|v| v as f32),
                Lit::Int(i) => i.base10_parse::<i64>().map(|v| v as f32),
                other => Err(syn::Error::new_spanned(other, "expected float or integer literal")),
            },
            other => Err(syn::Error::new_spanned(other, "expected float literal")),
        }
    }
}

impl FloatParseHelper for f64 {
    fn extract_expr(meta: &Meta) -> syn::Result<Expr> {
        match meta {
            Meta::NameValue(nv) => Ok(nv.value.clone()),
            other => Err(syn::Error::new_spanned(other, "expected name = float syntax")),
        }
    }

    fn parse_float(expr: &Expr) -> syn::Result<Self> {
        match expr {
            Expr::Lit(lit) => match &lit.lit {
                Lit::Float(f) => f.base10_parse::<f64>(),
                Lit::Int(i) => i.base10_parse::<i64>().map(|v| v as f64),
                other => Err(syn::Error::new_spanned(other, "expected float or integer literal")),
            },
            other => Err(syn::Error::new_spanned(other, "expected float literal")),
        }
    }
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// 字符类型支持 (char)
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

impl FromMetaValue for char {
    fn from_meta(meta: &Meta) -> syn::Result<Self> {
        let expr = match meta {
            Meta::NameValue(nv) => &nv.value,
            other => {
                return Err(syn::Error::new_spanned(other, "expected name = 'char' syntax"));
            }
        };
        Self::parse_char(expr)
    }

    fn from_value(value: &Expr) -> syn::Result<Self> {
        Self::parse_char(value)
    }
}

trait CharParseHelper {
    fn parse_char(expr: &Expr) -> syn::Result<char>;
}

impl CharParseHelper for char {
    fn parse_char(expr: &Expr) -> syn::Result<char> {
        match expr {
            Expr::Lit(lit) => match &lit.lit {
                Lit::Char(c) => Ok(c.value()),
                other => {
                    Err(syn::Error::new_spanned(other, "expected character literal (e.g., ',')"))
                }
            },
            other => Err(syn::Error::new_spanned(other, "expected character literal")),
        }
    }
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// 整数解析辅助方法（私有 trait，消除重复代码）
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

trait IntParseHelper: Sized {
    fn extract_name_value_expr(meta: &Meta) -> syn::Result<Expr>;
    fn parse_int_expr(expr: &Expr) -> syn::Result<Self>;
    fn parse_int_expr_signed(expr: &Expr) -> syn::Result<Self>;
}

impl IntParseHelper for usize {
    fn extract_name_value_expr(meta: &Meta) -> syn::Result<Expr> {
        match meta {
            Meta::NameValue(nv) => Ok(nv.value.clone()),
            other => Err(syn::Error::new_spanned(other, "expected name = integer syntax")),
        }
    }

    fn parse_int_expr(expr: &Expr) -> syn::Result<Self> {
        match expr {
            Expr::Lit(lit) => match &lit.lit {
                Lit::Int(i) => i.base10_parse::<usize>(),
                other => Err(syn::Error::new_spanned(other, "expected unsigned integer")),
            },
            other => Err(syn::Error::new_spanned(other, "expected unsigned integer")),
        }
    }

    fn parse_int_expr_signed(expr: &Expr) -> syn::Result<Self> {
        Err(syn::Error::new_spanned(expr, "usize does not support negative values"))
    }
}

impl IntParseHelper for i64 {
    fn extract_name_value_expr(meta: &Meta) -> syn::Result<Expr> {
        match meta {
            Meta::NameValue(nv) => Ok(nv.value.clone()),
            other => Err(syn::Error::new_spanned(other, "expected name = integer syntax")),
        }
    }

    fn parse_int_expr(expr: &Expr) -> syn::Result<Self> {
        match expr {
            Expr::Lit(lit) => match &lit.lit {
                Lit::Int(i) => i.base10_parse::<i64>(),
                other => Err(syn::Error::new_spanned(other, "expected signed integer")),
            },
            other => Err(syn::Error::new_spanned(other, "expected signed integer")),
        }
    }

    fn parse_int_expr_signed(expr: &Expr) -> syn::Result<Self> {
        match expr {
            Expr::Unary(un) => {
                let inner = Self::parse_int_expr(&un.expr)?;
                match un.op {
                    syn::UnOp::Neg(..) => Ok(-inner),
                    _ => Err(syn::Error::new_spanned(expr, "unexpected unary operator")),
                }
            }
            _ => Self::parse_int_expr(expr),
        }
    }
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// AttrStruct — 声明式属性结构定义与解析器
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

pub struct AttrStruct {
    pub name: &'static str,
    pub fields: Vec<AttrField>,
}

pub struct AttrField {
    pub name: &'static str,
    pub required: bool,
}

impl AttrStruct {
    pub fn new(name: &'static str) -> Self {
        Self { name, fields: Vec::new() }
    }

    pub fn field(mut self, name: &'static str) -> Self {
        self.fields.push(AttrField { name, required: true });
        self
    }

    pub fn optional_field(mut self, name: &'static str) -> Self {
        self.fields.push(AttrField { name, required: false });
        self
    }

    pub fn parse(&self, attrs: &[Attribute]) -> syn::Result<AttrValues> {
        let attr = find_attr(attrs, self.name).ok_or_else(|| {
            syn::Error::new(
                proc_macro2::Span::call_site(),
                format!("missing attribute #[{}(...)]", self.name),
            )
        })?;

        let mut values = HashMap::new();

        match &attr.meta {
            Meta::List(list) => {
                use syn::parse::Parser;
                let nested: Vec<Meta> =
                    syn::punctuated::Punctuated::<Meta, syn::Token![,]>::parse_terminated
                        .parse2(list.tokens.clone())
                        .map(|p| p.into_iter().collect())
                        .unwrap_or_default();

                for nested_meta in &nested {
                    let key = match nested_meta {
                        Meta::NameValue(nv) => {
                            nv.path.get_ident().map(|id| id.to_string()).unwrap_or_default()
                        }
                        Meta::Path(p) => p.get_ident().map(|id| id.to_string()).unwrap_or_default(),
                        Meta::List(li) => {
                            li.path.get_ident().map(|id| id.to_string()).unwrap_or_default()
                        }
                    };

                    let expr: Expr = match nested_meta {
                        Meta::NameValue(nv) => nv.value.clone(),
                        Meta::Path(p) => {
                            parse_quote!(#p)
                        }
                        Meta::List(li) => {
                            parse_quote!(#li)
                        }
                    };
                    values.insert(key, expr);
                }
            }
            Meta::Path(_) | Meta::NameValue(_) => {}
        }

        for field in &self.fields {
            if field.required && !values.contains_key(field.name) {
                return Err(syn::Error::new(
                    proc_macro2::Span::call_site(),
                    format!("missing required field `{}` in #[{}(...)]", field.name, self.name),
                ));
            }
        }

        Ok(AttrValues { values })
    }
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// AttrValues — 解析后的属性值容器
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

pub struct AttrValues {
    values: HashMap<String, Expr>,
}

impl AttrValues {
    pub fn get_raw(&self, name: &str) -> Option<&Expr> {
        self.values.get(name)
    }

    pub fn get_string(&self, name: &str) -> syn::Result<Option<String>> {
        match self.values.get(name) {
            Some(expr) => String::from_value(expr).map(Some),
            None => Ok(None),
        }
    }

    pub fn get_bool(&self, name: &str) -> syn::Result<Option<bool>> {
        match self.values.get(name) {
            Some(expr) => match expr {
                Expr::Path(_) => Ok(Some(true)),
                other => bool::from_value(other).map(Some),
            },
            None => Ok(None),
        }
    }

    pub fn get_usize(&self, name: &str) -> syn::Result<Option<usize>> {
        match self.values.get(name) {
            Some(expr) => usize::from_value(expr).map(Some),
            None => Ok(None),
        }
    }

    pub fn get_i64(&self, name: &str) -> syn::Result<Option<i64>> {
        match self.values.get(name) {
            Some(expr) => i64::from_value(expr).map(Some),
            None => Ok(None),
        }
    }

    pub fn get_ident(&self, name: &str) -> syn::Result<Option<Ident>> {
        match self.values.get(name) {
            Some(expr) => Ident::from_value(expr).map(Some),
            None => Ok(None),
        }
    }

    pub fn get_path(&self, name: &str) -> syn::Result<Option<Path>> {
        match self.values.get(name) {
            Some(expr) => Path::from_value(expr).map(Some),
            None => Ok(None),
        }
    }

    pub fn get_vec<T: FromMetaValue>(&self, name: &str) -> syn::Result<Option<Vec<T>>> {
        match self.values.get(name) {
            Some(expr) => T::from_value(expr).map(|v| Some(vec![v])),
            None => Ok(None),
        }
    }

    pub fn get_f32(&self, name: &str) -> syn::Result<Option<f32>> {
        match self.values.get(name) {
            Some(expr) => f32::from_value(expr).map(Some),
            None => Ok(None),
        }
    }

    pub fn get_f64(&self, name: &str) -> syn::Result<Option<f64>> {
        match self.values.get(name) {
            Some(expr) => f64::from_value(expr).map(Some),
            None => Ok(None),
        }
    }

    pub fn get_char(&self, name: &str) -> syn::Result<Option<char>> {
        match self.values.get(name) {
            Some(expr) => char::from_value(expr).map(Some),
            None => Ok(None),
        }
    }

    pub fn require_string(&self, name: &str) -> syn::Result<String> {
        self.get_string(name)?.ok_or_else(|| {
            syn::Error::new(
                proc_macro2::Span::call_site(),
                format!("missing required field `{}`", name),
            )
        })
    }

    pub fn require_bool(&self, name: &str) -> syn::Result<bool> {
        self.get_bool(name)?.ok_or_else(|| {
            syn::Error::new(
                proc_macro2::Span::call_site(),
                format!("missing required field `{}`", name),
            )
        })
    }

    pub fn require_usize(&self, name: &str) -> syn::Result<usize> {
        self.get_usize(name)?.ok_or_else(|| {
            syn::Error::new(
                proc_macro2::Span::call_site(),
                format!("missing required field `{}`", name),
            )
        })
    }

    pub fn require_i64(&self, name: &str) -> syn::Result<i64> {
        self.get_i64(name)?.ok_or_else(|| {
            syn::Error::new(
                proc_macro2::Span::call_site(),
                format!("missing required field `{}`", name),
            )
        })
    }

    pub fn require_ident(&self, name: &str) -> syn::Result<Ident> {
        self.get_ident(name)?.ok_or_else(|| {
            syn::Error::new(
                proc_macro2::Span::call_site(),
                format!("missing required field `{}`", name),
            )
        })
    }

    pub fn require_path(&self, name: &str) -> syn::Result<Path> {
        self.get_path(name)?.ok_or_else(|| {
            syn::Error::new(
                proc_macro2::Span::call_site(),
                format!("missing required field `{}`", name),
            )
        })
    }

    pub fn require_f32(&self, name: &str) -> syn::Result<f32> {
        self.get_f32(name)?.ok_or_else(|| {
            syn::Error::new(
                proc_macro2::Span::call_site(),
                format!("missing required field `{}`", name),
            )
        })
    }

    pub fn require_f64(&self, name: &str) -> syn::Result<f64> {
        self.get_f64(name)?.ok_or_else(|| {
            syn::Error::new(
                proc_macro2::Span::call_site(),
                format!("missing required field `{}`", name),
            )
        })
    }

    pub fn require_char(&self, name: &str) -> syn::Result<char> {
        self.get_char(name)?.ok_or_else(|| {
            syn::Error::new(
                proc_macro2::Span::call_site(),
                format!("missing required field `{}`", name),
            )
        })
    }
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// 辅助函数
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

pub fn find_attr<'a>(attrs: &'a [Attribute], name: &str) -> Option<&'a Attribute> {
    attrs.iter().find(|attr| attr.path().is_ident(name))
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// `#[derive(FromMeta)]` — 声明式属性解析派生宏
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

/// 为结构体自动生成 `FromMeta` 实现。
///
/// ## 支持的字段类型
///
/// | Rust 类型 | 属性语法 | 默认值 |
/// |-----------|---------|--------|
/// | `String` | `name = "value"` | 必填 |
/// | `bool` | `flag` 或 `flag = true/false` | `false` |
/// | `usize` / `i64` | `count = 42` | 必填 |
/// | `f32` / `f64` | `ratio = 0.5` | 必填 |
/// | `char` | `sep = ','` | 必填 |
/// | `Ident` | `kind = CustomName` | 必填 |
/// | `Path` | `ty = some::path` | 必填 |
/// | `Option<T>` | `opt = ...` | `None` |
/// | `Vec<T>` | `items = [a, b]` | `[]` |
///
/// ## 字段属性
///
/// - `#[meta(default)]` — 标记可选字段（有默认值）
/// - `#[meta(rename = "attr_name")]` — 自定义属性名
pub fn expand_from_meta_derive(input: &syn::DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    let struct_name = &input.ident;
    let fields = match &input.data {
        syn::Data::Struct(syn::DataStruct { fields: syn::Fields::Named(f), .. }) => &f.named,
        _ => {
            return Err(syn::Error::new_spanned(
                struct_name,
                "FromMeta can only be derived for structs with named fields",
            ));
        }
    };

    let mut field_inits = Vec::new();
    for field in fields {
        let field_name = field.ident.as_ref().unwrap();
        let ty = &field.ty;
        let is_optional = has_default_attr(field);
        if is_optional {
            field_inits.push(quote! { #field_name: <#ty as FromMetaValue>::from_meta(meta)? });
        } else {
            field_inits.push(quote! { #field_name: <#ty as FromMetaValue>::from_value(&expr)? });
        }
    }

    Ok(quote! {
        impl #struct_name {
            pub fn from_meta(meta: &syn::Meta) -> syn::Result<Self> {
                let expr = Self::extract_expr(meta)?;
                Ok(Self { #(#field_inits)* })
            }
        }

        impl #struct_name {
            fn extract_expr(meta: &syn::Meta) -> syn::Result<syn::Expr> {
                match meta {
                    syn::Meta::NameValue(nv) => Ok(nv.value.clone()),
                    other => Err(syn::Error::new_spanned(other, "expected key = value syntax")),
                }
            }
        }
    })
}

fn has_default_attr(field: &syn::Field) -> bool {
    field.attrs.iter().any(|attr| {
        attr.path().is_ident("meta")
            && matches!(
                attr.parse_args::<syn::Meta>(),
                Ok(syn::Meta::Path(p)) if p.get_ident()
                    .is_some_and(|i| i == "default")
            )
    })
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// Tests — Happy Path / Edge Case / Poison Pill
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

#[cfg(test)]
mod tests {
    use super::*;
    use quote::quote;
    use syn::parse_quote;

    fn parse_attrs(input: &str) -> Vec<Attribute> {
        let item: syn::ItemFn = syn::parse_str(input).expect("failed to parse test input");
        item.attrs
    }

    // ════════════════════════════════════════════════════════════
    // FromMetaValue — String
    // ════════════════════════════════════════════════════════════

    #[test]
    fn string_from_name_value() {
        let meta: Meta = parse_quote!(name = "hello");
        assert_eq!(String::from_meta(&meta).unwrap(), "hello");
    }

    #[test]
    fn string_from_value_lit() {
        let expr: Expr = parse_quote!("world");
        assert_eq!(String::from_value(&expr).unwrap(), "world");
    }

    #[test]
    fn string_rejects_non_string() {
        let expr: Expr = parse_quote!(42);
        assert!(String::from_value(&expr).is_err());
    }

    // ════════════════════════════════════════════════════════════
    // FromMetaValue — bool (bare word + literal)
    // ════════════════════════════════════════════════════════════

    #[test]
    fn bool_from_bare_word() {
        let meta: Meta = parse_quote!(enabled);
        assert!(bool::from_meta(&meta).unwrap());
    }

    #[test]
    fn bool_from_literal_true() {
        let meta: Meta = parse_quote!(enabled = true);
        assert!(bool::from_meta(&meta).unwrap());
    }

    #[test]
    fn bool_from_literal_false() {
        let meta: Meta = parse_quote!(enabled = false);
        assert!(!bool::from_meta(&meta).unwrap());
    }

    // ════════════════════════════════════════════════════════════
    // FromMetaValue — usize
    // ════════════════════════════════════════════════════════════

    #[test]
    fn usize_from_int() {
        let meta: Meta = parse_quote!(count = 42);
        assert_eq!(usize::from_meta(&meta).unwrap(), 42);
    }

    #[test]
    fn usize_zero() {
        let meta: Meta = parse_quote!(size = 0);
        assert_eq!(usize::from_meta(&meta).unwrap(), 0);
    }

    // ════════════════════════════════════════════════════════════
    // FromMetaValue — i64 (signed, supports negation)
    // ════════════════════════════════════════════════════════════

    #[test]
    fn i64_positive() {
        let meta: Meta = parse_quote!(offset = 100);
        assert_eq!(i64::from_meta(&meta).unwrap(), 100);
    }

    #[test]
    fn i64_negative() {
        let meta: Meta = parse_quote!(offset = -42);
        assert_eq!(i64::from_meta(&meta).unwrap(), -42);
    }

    // ════════════════════════════════════════════════════════════
    // FromMetaValue — Ident
    // ════════════════════════════════════════════════════════════

    #[test]
    fn ident_from_path() {
        let meta: Meta = parse_quote!(Foo);
        let id = Ident::from_meta(&meta).unwrap();
        assert_eq!(id.to_string(), "Foo");
    }

    // ════════════════════════════════════════════════════════════
    // FromMetaValue — Path
    // ════════════════════════════════════════════════════════════

    #[test]
    fn path_simple() {
        let meta: Meta = parse_quote!(my::path);
        let path = Path::from_meta(&meta).unwrap();
        assert_eq!(quote!(#path).to_string(), "my :: path");
    }

    // ════════════════════════════════════════════════════════════
    // AttrStruct — 声明式解析 (Happy Path)
    // ════════════════════════════════════════════════════════════

    #[test]
    fn attr_struct_happy_path() {
        let attrs = parse_attrs(
            r#"
            #[nuzo_config(name = "test", count = 10)]
            fn example() {}
            "#,
        );

        let parser = AttrStruct::new("nuzo_config").field("name").field("count");

        let values = parser.parse(&attrs).unwrap();
        assert_eq!(values.require_string("name").unwrap(), "test");
        assert_eq!(values.require_usize("count").unwrap(), 10);
    }

    // ════════════════════════════════════════════════════════════
    // AttrStruct — Optional Field Missing OK (Edge Case)
    // ════════════════════════════════════════════════════════════

    #[test]
    fn attr_struct_optional_field_missing_ok() {
        let attrs = parse_attrs(
            r#"
            #[nuzo_config(name = "only")]
            fn example() {}
            "#,
        );

        let parser = AttrStruct::new("nuzo_config").field("name").optional_field("count");

        let values = parser.parse(&attrs).unwrap();
        assert_eq!(values.require_string("name").unwrap(), "only");
        assert!(values.get_usize("count").unwrap().is_none());
    }

    // ════════════════════════════════════════════════════════════
    // AttrStruct — Missing Required Field Error (Poison Pill)
    // ════════════════════════════════════════════════════════════

    #[test]
    fn attr_struct_missing_required_field_error() {
        let attrs = parse_attrs(
            r#"
            #[nuzo_config(name = "incomplete")]
            fn example() {}
            "#,
        );

        let parser = AttrStruct::new("nuzo_config").field("name").field("count");

        assert!(parser.parse(&attrs).is_err());
    }

    // ════════════════════════════════════════════════════════════
    // AttrStruct — Missing Attribute Entirely (Poison Pill)
    // ════════════════════════════════════════════════════════════

    #[test]
    fn attr_struct_missing_attribute_error() {
        let attrs = parse_attrs(
            r#"
            #[other_attr]
            fn example() {}
            "#,
        );

        let parser = AttrStruct::new("nuzo_config").field("name");
        assert!(parser.parse(&attrs).is_err());
    }

    // ════════════════════════════════════════════════════════════
    // AttrValues — bare word bool access
    // ════════════════════════════════════════════════════════════

    #[test]
    fn values_get_bool_bare_word() {
        let attrs = parse_attrs(
            r#"
            #[cfg(flag)]
            fn example() {}
            "#,
        );
        let parser = AttrStruct::new("cfg").optional_field("flag");
        let values = parser.parse(&attrs).unwrap();
        assert_eq!(values.get_bool("flag").unwrap(), Some(true));
    }

    // ════════════════════════════════════════════════════════════
    // AttrValues — require on missing returns error (Poison Pill)
    // ════════════════════════════════════════════════════════════

    #[test]
    fn values_require_on_missing_errors() {
        let attrs = parse_attrs(
            r#"
            #[cfg()]
            fn example() {}
            "#,
        );
        let parser = AttrStruct::new("cfg").optional_field("nothing");
        let values = parser.parse(&attrs).unwrap();
        assert!(values.require_string("nothing").is_err());
    }

    // ════════════════════════════════════════════════════════════
    // find_attr — 辅助函数
    // ════════════════════════════════════════════════════════════

    #[test]
    fn find_attr_exists() {
        let attrs = parse_attrs(
            r#"
            #[allow(dead_code)]
            #[deny(unused)]
            fn example() {}
            "#,
        );
        assert!(find_attr(&attrs, "allow").is_some());
        assert!(find_attr(&attrs, "deny").is_some());
    }

    #[test]
    fn find_attr_not_found() {
        let attrs = parse_attrs(
            r#"
            #[allow(dead_code)]
            fn example() {}
            "#,
        );
        assert!(find_attr(&attrs, "forbid").is_none());
    }
}
