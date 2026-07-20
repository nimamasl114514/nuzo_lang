//! `define_builtins!` 核心展开逻辑
//!
//! 从声明式块式语法生成 builtin 函数的批量注册代码与文档常量。
//! 与 [`nuzo_codegen::opcode_gen`] 同构：双 crate 架构下，本模块仅负责展开逻辑，
//! `#[proc_macro]` 入口由 `nuzo_proc` 在 T13 中注册。
//!
//! ## 语法
//!
//! ```ignore
//! define_builtins! {
//!     /// 打印值（无换行）
//!     "print" => builtin_print, arity = 0,
//!         signature = "print(args...) -> nil",
//!         desc = "打印值（无换行）";
//!     /// 打印值（带换行）
//!     "println" => builtin_println, arity = 0,
//!         signature = "println(args...) -> nil",
//!         desc = "打印值（带换行）";
//! }
//! ```
//!
//! ## 展开产物
//!
//! 1. 一组 `reg.register(name, fn, arity as usize);` 调用
//! 2. `pub const DOMAIN_DOCS: &[BuiltinDoc] = &[ ... ];` 常量
//!
//! ## 设计要点
//!
//! - **块式语法**：每条以 `;` 结尾，属性以 `key = value` 形式给出，可读性高
//! - **doc comment 友好**：条目前的 `///` 文档注释会被收集，若未提供 `desc`
//!   则作为描述回退（双保险）
//! - **重名检测**：编译期 `syn::Error` 触发 `compile_error!`
//! - **arity 类型**：内部用 `u8`（与 `BuiltinRegistry::register` 的 `usize` 兼容，
//!   宏内 `as usize` 转换）

use proc_macro2::TokenStream;
use quote::quote;
use syn::parse::Parse;
use syn::{Ident, LitInt, LitStr, Token, parse::ParseStream};

/// 单条 Builtin 的文档信息（运行时反射 / 文档生成用）
///
/// 字段全部为 `&'static str` —— 由宏展开为静态常量数组，
/// 零运行时开销。
#[derive(Debug, Clone)]
pub struct BuiltinDoc {
    /// 函数名（Nuzo 源码中的调用名）
    pub name: &'static str,
    /// 参数数量（u8，与 BuiltinRegistry 内部存储一致）
    pub arity: u8,
    /// 函数签名（人类可读，如 `"print(args...) -> nil"`）
    pub signature: &'static str,
    /// 函数描述（来自 `desc = "..."` 或 `///` 文档注释回退）
    pub description: &'static str,
}

/// 单条 builtin 条目的解析结果
///
/// 对应语法：`"name" => fn_path, arity = N, signature = "...", desc = "...";`
#[derive(Debug)]
pub struct BuiltinEntry {
    pub name: String,
    pub fn_path: syn::Path,
    pub arity: u8,
    pub signature: String,
    pub desc: String,
    /// 条目前的 `///` 文档注释（按出现顺序，已剥离 `///` 前缀）
    pub doc_lines: Vec<String>,
}

impl BuiltinEntry {
    /// 取最终描述：优先 `desc`，为空则回退到 doc comment 拼接
    fn effective_desc(&self) -> String {
        if !self.desc.is_empty() {
            self.desc.clone()
        } else if !self.doc_lines.is_empty() {
            self.doc_lines.join("\n")
        } else {
            String::new()
        }
    }
}

impl Parse for BuiltinEntry {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        // 1. 解析条目前的 outer attributes（包含 `///` doc comment）
        //    `///` 在 syn 中被解析为 `#[doc = "..."]` 属性
        let attrs = input.call(syn::Attribute::parse_outer)?;
        let mut doc_lines: Vec<String> = Vec::new();
        for attr in &attrs {
            if attr.path().is_ident("doc")
                && let syn::Meta::NameValue(nv) = &attr.meta
                && let syn::Expr::Lit(syn::ExprLit { lit: syn::Lit::Str(s), .. }) = &nv.value
            {
                doc_lines.push(s.value().trim().to_string());
            }
        }

        let name: LitStr = input.parse()?;
        input.parse::<Token![=>]>()?;

        let fn_path: syn::Path = input.parse()?;

        let mut arity: Option<u8> = None;
        let mut signature = String::new();
        let mut desc = String::new();

        while !input.is_empty() && !input.peek(Token![;]) {
            input.parse::<Token![,]>()?;
            // 允许末尾多余逗号（如 `..., ;` 的情况）—— 再检查一次
            if input.is_empty() || input.peek(Token![;]) {
                break;
            }
            let key: Ident = input.parse()?;
            input.parse::<Token![=]>()?;
            let key_str = key.to_string();
            match key_str.as_str() {
                "arity" => {
                    if arity.is_some() {
                        return Err(syn::Error::new(key.span(), "重复属性: `arity` 已指定"));
                    }
                    let val: LitInt = input.parse()?;
                    let v: u8 = val.base10_parse()?;
                    arity = Some(v);
                }
                "signature" => {
                    if !signature.is_empty() {
                        return Err(syn::Error::new(key.span(), "重复属性: `signature` 已指定"));
                    }
                    let val: LitStr = input.parse()?;
                    signature = val.value();
                }
                "desc" => {
                    if !desc.is_empty() {
                        return Err(syn::Error::new(key.span(), "重复属性: `desc` 已指定"));
                    }
                    let val: LitStr = input.parse()?;
                    desc = val.value();
                }
                other => {
                    return Err(syn::Error::new(
                        key.span(),
                        format!("未知属性 `{other}`，预期为 `arity` / `signature` / `desc`"),
                    ));
                }
            }
        }

        input.parse::<Token![;]>()?;

        let arity = arity.ok_or_else(|| {
            syn::Error::new(
                proc_macro2::Span::call_site(),
                format!("builtin `{}` 缺少必填属性 `arity`", name.value()),
            )
        })?;

        Ok(BuiltinEntry { name: name.value(), fn_path, arity, signature, desc, doc_lines })
    }
}

/// 整个 `define_builtins! { ... }` 块的解析结果
#[derive(Debug)]
pub struct BuiltinEntries(pub Vec<BuiltinEntry>);

impl Parse for BuiltinEntries {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut entries = Vec::new();
        while !input.is_empty() {
            entries.push(input.parse::<BuiltinEntry>()?);
        }
        Ok(BuiltinEntries(entries))
    }
}

/// 展开 `define_builtins!` 宏
///
/// 接受 `proc_macro2::TokenStream` 以便在单元测试中直接调用（无需 proc-macro 上下文），
/// 与 [`nuzo_codegen::opcode_gen::expand_define_opcodes`] 保持一致的接口约定。
///
/// # 错误
///
/// - 重名 builtin：返回 `syn::Error`，由调用方触发 `compile_error!`
/// - 缺少必填属性 `arity`：在 [`BuiltinEntry::parse`] 中已报错
pub fn expand_define_builtins(input: TokenStream) -> syn::Result<TokenStream> {
    let BuiltinEntries(entries) = syn::parse2::<BuiltinEntries>(input)?;

    // 重名检测：用 HashSet 收集已见 name，首次重复即报错（span 指向重复条目本身，
    // 由于 Parse 阶段已消费 span，这里用 call_site 兜底）
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for e in &entries {
        if !seen.insert(e.name.clone()) {
            return Err(syn::Error::new(
                proc_macro2::Span::call_site(),
                format!("重复注册 builtin: `{}`", e.name),
            ));
        }
    }

    let register_calls: Vec<TokenStream> = entries
        .iter()
        .map(|e| {
            let name = &e.name;
            let fn_path = &e.fn_path;
            let arity = e.arity;
            // arity as usize：与 BuiltinRegistry::register 签名对齐
            quote! {
                reg.register(#name, #fn_path, #arity as usize);
            }
        })
        .collect();

    let docs_entries: Vec<TokenStream> = entries
        .iter()
        .map(|e| {
            let name = &e.name;
            let arity = e.arity;
            let signature = &e.signature;
            let description = e.effective_desc();
            quote! {
                ::nuzo_codegen::builtin_gen::BuiltinDoc {
                    name: #name,
                    arity: #arity,
                    signature: #signature,
                    description: #description,
                },
            }
        })
        .collect();

    Ok(quote! {
        #(#register_calls)*

        pub const DOMAIN_DOCS: &[::nuzo_codegen::builtin_gen::BuiltinDoc] = &[
            #(#docs_entries)*
        ];
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use quote::quote;

    #[test]
    fn parse_single_entry_minimal() {
        let input = quote! {
            "print" => builtin_print, arity = 0, signature = "print() -> nil", desc = "打印";
        };
        let entries = syn::parse2::<BuiltinEntries>(input).expect("解析失败");
        assert_eq!(entries.0.len(), 1);
        let e = &entries.0[0];
        assert_eq!(e.name, "print");
        assert_eq!(e.arity, 0);
        assert_eq!(e.signature, "print() -> nil");
        assert_eq!(e.desc, "打印");
    }

    #[test]
    fn parse_multiple_entries() {
        let input = quote! {
            "print" => builtin_print, arity = 0, signature = "print()", desc = "a";
            "println" => builtin_println, arity = 0, signature = "println()", desc = "b";
        };
        let entries = syn::parse2::<BuiltinEntries>(input).expect("解析失败");
        assert_eq!(entries.0.len(), 2);
    }

    #[test]
    fn doc_comment_falls_back_to_desc() {
        let input = quote! {
            /// 打印值
            "print" => builtin_print, arity = 0, signature = "print()", desc = "";
        };
        let entries = syn::parse2::<BuiltinEntries>(input).expect("解析失败");
        let e = &entries.0[0];
        assert_eq!(e.doc_lines, vec!["打印值".to_string()]);
        assert_eq!(e.effective_desc(), "打印值");
    }

    #[test]
    fn duplicate_name_errors() {
        let input = quote! {
            "print" => builtin_print, arity = 0, signature = "a", desc = "a";
            "print" => builtin_println, arity = 0, signature = "b", desc = "b";
        };
        let result = expand_define_builtins(input);
        let err = result.expect_err("应报重名错误");
        assert!(err.to_string().contains("重复注册 builtin"));
    }

    #[test]
    fn missing_arity_errors() {
        let input = quote! {
            "print" => builtin_print, signature = "a", desc = "a";
        };
        let result = syn::parse2::<BuiltinEntries>(input);
        let err = result.expect_err("应报缺少 arity");
        assert!(err.to_string().contains("缺少必填属性 `arity`"));
    }

    #[test]
    fn expand_generates_register_and_docs() {
        let input = quote! {
            "print" => builtin_print, arity = 0, signature = "print()", desc = "打印";
        };
        let output = expand_define_builtins(input).expect("展开失败");
        let s = output.to_string();
        assert!(s.contains("reg . register"), "应生成 reg.register 调用: {s}");
        assert!(s.contains("DOMAIN_DOCS"), "应生成 DOMAIN_DOCS 常量: {s}");
        assert!(s.contains("BuiltinDoc"), "应引用 BuiltinDoc 类型: {s}");
    }
}
