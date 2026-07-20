//! `define_opcodes!` 核心展开逻辑
//!
//! 从声明式属性定义生成完整的 Opcode 枚举及其所有方法实现。
//! 原本位于 `nuzo_proc/src/lib.rs`，迁移到此处以符合双 crate 架构。
//!
//! ## 设计哲学
//!
//! 旧版 `macro_rules! define_opcodes!` 使用位置参数语法，可读性差且难以扩展。
//! 新版采用**属性宏风格**（类似 derive + 属性组合），将元数据收敛到 `#[opcode(...)]`
//! 中，使每条指令的定义自包含、IDE 友好、未来可扩展。

use proc_macro2::Ident;
use quote::quote;
use syn::{Token, parse::Parse, parse::ParseStream, punctuated::Punctuated};

/// 单条操作码的完整定义（解析后）。
pub struct OpcodeDef {
    pub ident: Ident,
    pub doc_attrs: Vec<syn::Attribute>,
    pub code: u8,
    pub size: usize,
    pub operands: Vec<Ident>,
    pub disasm: OptionDisasm,
    pub dispatch: DispatchKindVal,
    pub desc: String,
    pub summary: String,
}

/// `disasm` 字段的可选值。
pub enum OptionDisasm {
    Str(String),
    Custom,
}

impl OptionDisasm {
    pub fn to_tokens(&self) -> proc_macro2::TokenStream {
        match self {
            OptionDisasm::Str(s) => {
                let lit = syn::LitStr::new(s, proc_macro2::Span::call_site());
                quote!(Some(#lit))
            }
            OptionDisasm::Custom => quote!(None),
        }
    }
}

/// `dispatch` 字段的可选值。
pub enum DispatchKindVal {
    Known(syn::Path),
    Default,
}

impl DispatchKindVal {
    pub fn to_tokens(&self) -> proc_macro2::TokenStream {
        match self {
            DispatchKindVal::Known(path) => {
                quote!(DispatchKind::#path)
            }
            DispatchKindVal::Default => {
                quote!(DispatchKind::Custom)
            }
        }
    }
}

/// `#[opcode(...)]` 属性的解析结果。
pub struct OpcodeAttr {
    pub code: u8,
    pub size: usize,
    pub operands: Vec<Ident>,
    pub disasm: OptionDisasm,
    pub dispatch: DispatchKindVal,
    pub desc: String,
    pub summary: String,
}

impl Parse for OpcodeAttr {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        use nuzo_proc_core::parse_utils::{self as pu, parse_string_lit};

        let meta_items = Punctuated::<syn::Meta, Token![,]>::parse_terminated(input)?;

        let mut code: Option<u8> = None;
        let mut size: Option<usize> = None;
        let mut operands: Vec<Ident> = Vec::new();
        let mut disasm: Option<OptionDisasm> = None;
        let mut dispatch: Option<DispatchKindVal> = None;
        let mut desc: Option<String> = None;
        let mut summary: Option<String> = None;

        for meta in &meta_items {
            if let syn::Meta::NameValue(nv) = meta {
                if nv.path.is_ident("code") {
                    if code.is_some() {
                        return Err(dup_field_err(&nv.path, "code"));
                    }
                    code = Some(pu::parse_int_lit(&nv.value, "code", " (0..=255)")?);
                } else if nv.path.is_ident("size") {
                    if size.is_some() {
                        return Err(dup_field_err(&nv.path, "size"));
                    }
                    size = Some(pu::parse_int_lit(&nv.value, "size", "")?);
                } else if nv.path.is_ident("operands") {
                    if !operands.is_empty() {
                        return Err(dup_field_err(&nv.path, "operands"));
                    }
                    operands = pu::parse_operand_list(&nv.value)?;
                } else if nv.path.is_ident("disasm") {
                    if disasm.is_some() {
                        return Err(dup_field_err(&nv.path, "disasm"));
                    }
                    disasm = Some(parse_disasm_value(&nv.value)?);
                } else if nv.path.is_ident("dispatch") {
                    if dispatch.is_some() {
                        return Err(dup_field_err(&nv.path, "dispatch"));
                    }
                    dispatch = Some(parse_dispatch_kind(&nv.value)?);
                } else if nv.path.is_ident("desc") {
                    if desc.is_some() {
                        return Err(dup_field_err(&nv.path, "desc"));
                    }
                    desc = Some(parse_string_lit(&nv.value)?);
                } else if nv.path.is_ident("summary") {
                    if summary.is_some() {
                        return Err(dup_field_err(&nv.path, "summary"));
                    }
                    summary = Some(parse_string_lit(&nv.value)?);
                } else {
                    return Err(nuzo_proc_core::diag::SpannedError::new_spanned(
                        meta,
                        format!(
                            "unknown opcode attribute `{}`; \
                             allowed: code, size, operands, disasm, dispatch, desc, summary",
                            quote!(#meta)
                        ),
                    )
                    .into_inner());
                }
            } else {
                return Err(nuzo_proc_core::diag::SpannedError::new_spanned(
                    meta,
                    "expected `key = value` format in #[opcode(...)]",
                )
                .into_inner());
            }
        }

        let code = code.ok_or_else(|| missing_req_attr("code"))?;
        let size = size.ok_or_else(|| missing_req_attr("size"))?;

        Ok(OpcodeAttr {
            code,
            size,
            operands,
            disasm: disasm.unwrap_or(OptionDisasm::Custom),
            dispatch: dispatch.unwrap_or(DispatchKindVal::Default),
            desc: desc.unwrap_or_default(),
            summary: summary.unwrap_or_default(),
        })
    }
}

/// 输入顶层：支持可选 `(name = EnumName;)` 前缀 + 操作码条目列表。
pub struct OpcodeMacroInput {
    pub name: Ident,
    pub items: Vec<OpcodeItem>,
}

impl Parse for OpcodeMacroInput {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let name = if input.peek(syn::token::Paren) {
            let content;
            syn::parenthesized!(content in input);
            content.parse::<Ident>()?; // "name"
            content.parse::<Token![=]>()?;
            let ident: Ident = content.parse()?;
            content.parse::<Token![;]>()?;
            if content.peek(Token![,]) {
                content.parse::<Token![,]>()?;
            }
            ident
        } else {
            Ident::new("Opcode", proc_macro2::Span::call_site())
        };

        let items = Punctuated::<OpcodeItem, Token![,]>::parse_terminated(input)?;
        Ok(OpcodeMacroInput { name, items: items.into_iter().collect() })
    }
}

/// 单个操作码条目：属性 + 标识符。
pub struct OpcodeItem {
    pub attrs: Vec<syn::Attribute>,
    pub ident: Ident,
}

impl Parse for OpcodeItem {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let attrs = input.call(syn::Attribute::parse_outer)?;
        let ident: Ident = input.parse()?;
        Ok(OpcodeItem { attrs, ident })
    }
}

/// 从条目列表中提取并验证 OpcodeDef 列表。
pub fn parse_opcode_defs(items: Vec<OpcodeItem>) -> syn::Result<Vec<OpcodeDef>> {
    let mut defs = Vec::with_capacity(items.len());

    for item in items {
        let OpcodeItem { attrs, ident } = item;

        let mut doc_attrs = Vec::new();
        let mut opcode_attr: Option<&syn::Attribute> = None;

        for attr in &attrs {
            if attr.path().is_ident("opcode") {
                if opcode_attr.is_some() {
                    return Err(nuzo_proc_core::diag::SpannedError::new_spanned(
                        attr,
                        "duplicate `#[opcode(...)]` attribute on same variant",
                    )
                    .into_inner());
                }
                opcode_attr = Some(attr);
            } else if attr.path().is_ident("doc") {
                doc_attrs.push(attr.clone());
            }
        }

        let opcode_attr = opcode_attr.ok_or_else(|| {
            nuzo_proc_core::diag::SpannedError::new_spanned(
                &ident,
                format!("missing `#[opcode(...)]` attribute on `{}`", ident),
            )
            .into_inner()
        })?;

        let attr: OpcodeAttr = opcode_attr.parse_args()?;

        defs.push(OpcodeDef {
            ident,
            doc_attrs,
            code: attr.code,
            size: attr.size,
            operands: attr.operands,
            disasm: attr.disasm,
            dispatch: attr.dispatch,
            desc: attr.desc,
            summary: attr.summary,
        });
    }

    check_duplicate_codes(&defs)?;
    Ok(defs)
}

/// 检查操作码数值是否有重复。
fn check_duplicate_codes(defs: &[OpcodeDef]) -> syn::Result<()> {
    let mut seen = std::collections::HashSet::new();
    for def in defs {
        if !seen.insert(def.code) {
            return Err(nuzo_proc_core::diag::SpannedError::new_spanned(
                &def.ident,
                format!("duplicate opcode code `{}`", def.code),
            )
            .into_inner());
        }
    }
    Ok(())
}

/// 核心代码生成：产出完整的 enum + impl + const 校验。
pub fn generate_opcode_code(
    name: Ident,
    defs: Vec<OpcodeDef>,
) -> syn::Result<proc_macro2::TokenStream> {
    use nuzo_proc_core::parse_utils::operand_byte_size;
    use proc_macro2::TokenStream;

    let enum_variants: Vec<TokenStream> = defs
        .iter()
        .map(|d| {
            let ident = &d.ident;
            let code = d.code;
            let docs = &d.doc_attrs;
            quote!(#(#docs)* #ident = #code,)
        })
        .collect();

    let name_arms: Vec<TokenStream> = defs
        .iter()
        .map(|d| {
            let ident = &d.ident;
            let name_lit = syn::LitStr::new(&ident.to_string(), ident.span());
            quote!(Self::#ident => #name_lit,)
        })
        .collect();

    let size_arms: Vec<TokenStream> = defs
        .iter()
        .map(|d| {
            let ident = &d.ident;
            let size = d.size;
            quote!(Self::#ident => #size,)
        })
        .collect();

    let decode_arms: Vec<TokenStream> = defs
        .iter()
        .map(|d| {
            let ident = &d.ident;
            let code = d.code;
            quote!(#code => Some(Self::#ident),)
        })
        .collect();

    let operand_arms: Vec<TokenStream> = defs
        .iter()
        .map(|d| {
            let ident = &d.ident;
            let opers: Vec<TokenStream> =
                d.operands.iter().map(|op| quote!(OperandKind::#op,)).collect();
            quote!(Self::#ident => &[#(#opers)*],)
        })
        .collect();

    let disasm_arms: Vec<TokenStream> = defs
        .iter()
        .map(|d| {
            let ident = &d.ident;
            let tokens = d.disasm.to_tokens();
            quote!(Self::#ident => #tokens,)
        })
        .collect();

    let dispatch_arms: Vec<TokenStream> = defs
        .iter()
        .map(|d| {
            let ident = &d.ident;
            let tokens = d.dispatch.to_tokens();
            quote!(Self::#ident => #tokens,)
        })
        .collect();

    let desc_arms: Vec<TokenStream> = defs
        .iter()
        .map(|d| {
            let ident = &d.ident;
            let lit = syn::LitStr::new(&d.desc, d.ident.span());
            quote!(Self::#ident => #lit,)
        })
        .collect();

    let summary_arms: Vec<TokenStream> = defs
        .iter()
        .map(|d| {
            let ident = &d.ident;
            let lit = syn::LitStr::new(&d.summary, d.ident.span());
            quote!(Self::#ident => #lit,)
        })
        .collect();

    // OPCODE_DOCS 文档表条目（供 build.rs 生成 markdown 文档）
    let docs_arms: Vec<TokenStream> = defs
        .iter()
        .map(|d| {
            let ident = &d.ident;
            let name_lit = syn::LitStr::new(&ident.to_string(), ident.span());
            let code = d.code;
            // 操作数列表转逗号分隔字符串（避免引用 OperandKind 造成循环依赖）
            let operands_str =
                d.operands.iter().map(|op| op.to_string()).collect::<Vec<_>>().join(", ");
            let operands_lit = syn::LitStr::new(&operands_str, ident.span());
            let desc_lit = syn::LitStr::new(&d.desc, ident.span());
            let summary_lit = syn::LitStr::new(&d.summary, ident.span());
            quote! {
                ::nuzo_proc_core::doc_sync::OpcodeDoc {
                    name: #name_lit,
                    code: #code,
                    operands: #operands_lit,
                    desc: #desc_lit,
                    summary: #summary_lit,
                },
            }
        })
        .collect();

    let all_idents: Vec<TokenStream> = defs
        .iter()
        .map(|d| {
            let ident = &d.ident;
            quote!(Self::#ident,)
        })
        .collect();

    let size_checks: Vec<TokenStream> = defs
        .iter()
        .map(|d| {
            let declared_size = d.size;
            let computed_size: usize = 1 + d
                .operands
                .iter()
                .map(|op| operand_byte_size(&op.to_string()))
                .collect::<syn::Result<Vec<usize>>>()?
                .iter()
                .sum::<usize>();
            Ok(quote!(const _: [(); #declared_size] = [(); #computed_size];))
        })
        .collect::<syn::Result<Vec<_>>>()?;

    Ok(quote! {
        /// VM 操作码枚举。由 `nuzo_proc::define_opcodes!` 自动生成。
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        #[cfg_attr(feature = "serde", derive(serde::Serialize))]
        #[repr(u8)]
        pub enum #name {
            #(#enum_variants)*
        }

        impl #name {
            #[inline]
            pub const fn name(self) -> &'static str {
                match self { #(#name_arms)* }
            }

            #[inline]
            pub const fn instruction_size(self) -> usize {
                match self { #(#size_arms)* }
            }

            #[inline]
            pub const fn decode_opcode(byte: u8) -> Option<Self> {
                match byte { #(#decode_arms)* _ => None, }
            }

            #[inline]
            pub fn operands(self) -> &'static [OperandKind] {
                match self { #(#operand_arms)* }
            }

            #[inline]
            pub fn disasm_template(self) -> Option<&'static str> {
                match self { #(#disasm_arms)* }
            }

            #[inline]
            pub const fn dispatch_kind(self) -> DispatchKind {
                match self { #(#dispatch_arms)* }
            }

            #[inline]
            pub const fn description(self) -> &'static str {
                match self { #(#desc_arms)* }
            }

            #[inline]
            pub const fn operand_summary(self) -> &'static str {
                match self { #(#summary_arms)* }
            }

            pub fn iter_all() -> impl Iterator<Item = Self> {
                [#(#all_idents)*].into_iter()
            }

            pub const ALL: &[Self] = &[#(#all_idents)*];
        }

        impl ::std::fmt::Display for #name {
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                f.write_str(self.name())
            }
        }

        /// Opcode 文档表常量（供 build.rs 生成 markdown 文档）。
        ///
        /// 由 `define_opcodes!` 宏自动生成，包含所有 Opcode 的结构化文档信息。
        pub const OPCODE_DOCS: &[::nuzo_proc_core::doc_sync::OpcodeDoc] = &[
            #(#docs_arms)*
        ];

        #(#size_checks)*
    })
}

/// 入口：解析 + 展开完整流程。
///
/// 接受 `proc_macro2::TokenStream` 以便在单元测试中直接调用（无需 proc-macro 上下文）。
pub fn expand_define_opcodes(
    input: proc_macro2::TokenStream,
) -> syn::Result<proc_macro2::TokenStream> {
    let parsed = syn::parse2::<OpcodeMacroInput>(input)?;
    let defs = parse_opcode_defs(parsed.items)?;
    generate_opcode_code(parsed.name, defs)
}

fn dup_field_err(path: &syn::Path, field: &str) -> syn::Error {
    nuzo_proc_core::diag::SpannedError::new_spanned(path, format!("duplicate `{field}` field"))
        .into_inner()
}

fn missing_req_attr(field: &str) -> syn::Error {
    nuzo_proc_core::diag::SpannedError::new(
        proc_macro2::Span::call_site(),
        format!("missing required attribute `{field}` in #[opcode(...)]"),
    )
    .into_inner()
}

fn parse_disasm_value(expr: &syn::Expr) -> syn::Result<OptionDisasm> {
    if let syn::Expr::Lit(lit) = expr
        && let syn::Lit::Str(s) = &lit.lit
    {
        return Ok(OptionDisasm::Str(s.value()));
    }
    if let syn::Expr::Path(p) = expr
        && p.path.is_ident("custom")
    {
        return Ok(OptionDisasm::Custom);
    }
    Err(nuzo_proc_core::diag::SpannedError::new_spanned(
        expr,
        "`disasm` must be a string literal (e.g., \"halt\") or the identifier `custom`",
    )
    .into_inner())
}

fn parse_dispatch_kind(expr: &syn::Expr) -> syn::Result<DispatchKindVal> {
    let p = nuzo_proc_core::parse_utils::parse_ident_path(expr)?;
    Ok(DispatchKindVal::Known(p))
}
