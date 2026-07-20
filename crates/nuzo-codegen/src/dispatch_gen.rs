//! `define_dispatch_auto!` 核心展开逻辑
//!
//! 通过 CamelCase → snake_case 命名约定自动推导 opcode handler 名称，
//! 生成 `INSTRUCTION_COUNT` 常量和 `get_handler()` 函数。

use proc_macro2::Ident;
use quote::{format_ident, quote};
use syn::{Token, parse::Parse, parse::ParseStream, punctuated::Punctuated};

/// 单条 dispatch 条目。
pub enum DispatchEntry {
    Auto(Ident),
    Explicit { opcode: Ident, handler: Ident },
}

/// 顶层输入：逗号分隔的 dispatch 条目列表。
pub struct DispatchInput {
    pub entries: Vec<DispatchEntry>,
}

impl Parse for DispatchInput {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let items = Punctuated::<DispatchItem, Token![,]>::parse_terminated(input)?;
        Ok(DispatchInput { entries: items.into_iter().map(|item| item.entry).collect() })
    }
}

/// 解析层条目。
struct DispatchItem {
    entry: DispatchEntry,
}

impl Parse for DispatchItem {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let opcode: Ident = input.parse()?;
        if input.peek(Token![=>]) {
            let _: Token![=>] = input.parse()?;
            let handler: Ident = input.parse()?;
            Ok(DispatchItem { entry: DispatchEntry::Explicit { opcode, handler } })
        } else {
            Ok(DispatchItem { entry: DispatchEntry::Auto(opcode) })
        }
    }
}

/// 展开核心：生成 INSTRUCTION_COUNT + get_handler()。
pub fn expand_define_dispatch_auto(
    input: proc_macro2::TokenStream,
) -> syn::Result<proc_macro2::TokenStream> {
    let parsed = syn::parse2::<DispatchInput>(input)?;

    let mut opcode_idents = Vec::new();
    let mut handler_idents = Vec::new();

    for entry in &parsed.entries {
        match entry {
            DispatchEntry::Auto(opcode) => {
                let handler_name =
                    nuzo_proc_core::parse_utils::camel_to_snake_op(&opcode.to_string());
                let handler = format_ident!("{}", handler_name);
                opcode_idents.push(opcode.clone());
                handler_idents.push(handler);
            }
            DispatchEntry::Explicit { opcode, handler } => {
                opcode_idents.push(opcode.clone());
                handler_idents.push(handler.clone());
            }
        }
    }

    let count_block: Vec<proc_macro2::TokenStream> =
        opcode_idents.iter().map(|op| quote!(let _ = Opcode::#op; count += 1;)).collect();

    let match_arms: Vec<proc_macro2::TokenStream> = opcode_idents
        .iter()
        .zip(handler_idents.iter())
        .map(|(op, h)| quote!(Opcode::#op => Some(#h),))
        .collect();

    Ok(quote! {
        pub const INSTRUCTION_COUNT: usize = {
            let mut count = 0;
            #(#count_block)*
            count
        };

        #[inline]
        #[allow(unreachable_patterns)]
        pub fn get_handler(opcode: Opcode) -> Option<OpHandler> {
            match opcode {
                #(#match_arms)*
                _ => None,
            }
        }
    })
}
