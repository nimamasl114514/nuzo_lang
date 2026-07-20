//! bind! 宏输入参数解析器
//!
//! 解析 `bind!("crate::module")` / `bind!(pub "crate::module as alias")` 等语法形式。

use proc_macro2::TokenStream;
use syn::parse::{Parse, ParseStream};
use syn::{Ident, LitStr, Token, Visibility};

// ---------------------------------------------------------------------------
// 数据结构
// ---------------------------------------------------------------------------

/// bind! 宏的输入参数集合
#[derive(Debug)]
pub struct BindArgs {
    /// 可选的可见性前缀: `pub` / `pub(crate)` / `pub(super)` / `pub(in path)`
    pub vis: Option<Visibility>,
    /// 绑定规格列表（至少一个）
    pub specs: Vec<BindSpec>,
}

/// 单条绑定规格
#[derive(Debug, Clone)]
pub enum BindSpec {
    Import {
        /// 导入路径，如 `crate::foo::bar`
        path: syn::Path,
        /// 别名，如 `as foo` 中的 `foo`
        alias: Option<Ident>,
        /// 是否为通配符导入（路径末尾为 `*`）
        glob: bool,
    },
}

// ---------------------------------------------------------------------------
// Parse 实现
// ---------------------------------------------------------------------------

impl Parse for BindArgs {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        // 1. 解析可选的可见性前缀
        let vis = if input.peek(Token![pub]) { Some(input.parse()?) } else { None };

        // 2. 循环解析逗号分隔的 LitStr → BindSpec
        let mut specs: Vec<BindSpec> = Vec::new();
        let mut first = true;

        loop {
            if input.is_empty() {
                break;
            }

            if !first {
                // 消费逗号分隔符
                if input.peek(Token![,]) {
                    input.parse::<Token![,]>()?;
                } else {
                    break;
                }
            }
            first = false;

            // vis 仅允许出现在最开头，中途出现报错
            if input.peek(Token![pub]) {
                return Err(syn::Error::new(input.span(), "可见性修饰符仅允许在 bind! 最开头"));
            }

            let lit: LitStr = input.parse()?;
            specs.push(parse_bind_spec(&lit)?);
        }

        // 3. specs 必须非空
        if specs.is_empty() {
            return Err(syn::Error::new(
                input.span(),
                "bind! 不能接受空输入; 用法: bind!(\"crate::module\")",
            ));
        }

        Ok(BindArgs { vis, specs })
    }
}

// ---------------------------------------------------------------------------
// 便捷入口
// ---------------------------------------------------------------------------

/// 解析 `bind!` 宏的输入 TokenStream，返回结构化参数。
pub fn parse_bind_args(input: TokenStream) -> syn::Result<BindArgs> {
    syn::parse2(input)
}

// ===========================================================================
// 内部辅助
// ===========================================================================

/// 将单个 LitStr 解析为 [`BindSpec`]。
fn parse_bind_spec(lit: &LitStr) -> syn::Result<BindSpec> {
    let raw = lit.value();
    let trimmed = raw.trim();

    if trimmed.is_empty() {
        return Err(syn::Error::new(lit.span(), "路径不能为空"));
    }

    // 1. 判定 glob（末尾 ::*）
    let glob = trimmed.ends_with("::*");
    let base = if glob { trimmed.strip_suffix("::*").unwrap().trim() } else { trimmed };

    // 2. 判定 alias（含 " as ident"）
    let (path_str, alias) = if let Some(pos) = base.rfind(" as ") {
        let path_part = base[..pos].trim();
        let alias_raw = base[pos + 4..].trim();

        if alias_raw.is_empty() {
            return Err(syn::Error::new(
                lit.span(),
                "as 后需要别名标识符, 例如 \"crate::foo as f\"",
            ));
        }

        let alias_ident = syn::parse_str::<Ident>(alias_raw)
            .map_err(|_| syn::Error::new(lit.span(), format!("无效的别名标识符: '{alias_raw}'")))?;

        (path_part.to_string(), Some(alias_ident))
    } else {
        (base.to_string(), None)
    };

    // 3. glob 与 alias 互斥
    if glob && alias.is_some() {
        return Err(syn::Error::new(lit.span(), "通配符导入 '*' 不支持别名重命名"));
    }

    if path_str.is_empty() {
        return Err(syn::Error::new(lit.span(), "路径不能为空"));
    }

    // 4. 解析 syn::Path
    let path = syn::parse_str::<syn::Path>(&path_str).map_err(|e| {
        syn::Error::new(lit.span(), format!("路径语法错误: {e}; 格式: \"crate::module_path\""))
    })?;

    Ok(BindSpec::Import { path, alias, glob })
}
