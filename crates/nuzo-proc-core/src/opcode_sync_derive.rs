//! `#[derive(OpcodeSync)]` 核心展开逻辑
//!
//! 从 `Instruction` 枚举自动生成 SSOT 宏和 dispatch 列表，
//! 消除 `with_every_instruction!` 和 `define_dispatch_auto!` 列表的手写重复。
//!
//! ## 支持的属性
//!
//! ### 枚举级 `#[opcode_meta(extra_dispatch = [...])]`
//!
//! 声明不在 `Instruction` 枚举中但需要 dispatch handler 的 `Opcode` 变体
//! （如 VM 内部 patch 出来的 `GetGlobalCached`、未来扩展的 `SpillLoad/SpillStore`）。
//!
//! ### 变体级 `#[opcode_meta(skip_ssot)]` / `#[opcode_meta(skip_dispatch)]`
//!
//! - `skip_ssot` — 不纳入 `with_every_instruction!` SSOT（如 `Halt`、`Capture`）
//! - `skip_dispatch` — 不纳入 `with_every_dispatch_opcode!` 列表
//!
//! ## 生成内容
//!
//! 1. `with_every_instruction!` — SSOT 宏（替代手写）
//! 2. `with_every_dispatch_opcode!` — dispatch 列表宏（供 `define_dispatch_auto!` 使用）
//! 3. `INSTRUCTION_COUNT` — 指令总数常量
//! 4. 编译期断言 — 防止手动修改常量

use proc_macro2::{Ident, TokenStream};
use quote::quote;
use syn::{Token, parse::Parse, parse::ParseStream, punctuated::Punctuated};

// ══════════════════════════════════════════════════════════════════
// 属性解析
// ══════════════════════════════════════════════════════════════════

/// 变体级 `#[opcode_meta(...)]` 属性。
///
/// 仅支持 flag 形式（无值），用于控制变体是否纳入 SSOT / dispatch 列表。
#[derive(Default)]
pub struct VariantMeta {
    pub skip_ssot: bool,
    pub skip_dispatch: bool,
}

impl Parse for VariantMeta {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let meta_items = Punctuated::<syn::Meta, Token![,]>::parse_terminated(input)?;
        let mut meta = VariantMeta::default();
        for item in &meta_items {
            match item {
                syn::Meta::Path(p) => {
                    if p.is_ident("skip_ssot") {
                        meta.skip_ssot = true;
                    } else if p.is_ident("skip_dispatch") {
                        meta.skip_dispatch = true;
                    } else {
                        return Err(syn::Error::new_spanned(
                            p,
                            "unknown `opcode_meta` flag; allowed: skip_ssot, skip_dispatch",
                        ));
                    }
                }
                _ => {
                    return Err(syn::Error::new_spanned(
                        item,
                        "expected flag (e.g., `skip_ssot`) in `#[opcode_meta(...)]`",
                    ));
                }
            }
        }
        Ok(meta)
    }
}

/// 枚举级 `#[opcode_meta(extra_dispatch = [...])]` 属性。
///
/// 用于声明不在 `Instruction` 枚举中但需要 dispatch handler 的 `Opcode` 变体。
pub struct EnumMeta {
    pub extra_dispatch: Vec<Ident>,
}

impl Parse for EnumMeta {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let meta_items = Punctuated::<syn::Meta, Token![,]>::parse_terminated(input)?;
        let mut extra_dispatch = Vec::new();
        for item in &meta_items {
            if let syn::Meta::NameValue(nv) = item {
                if nv.path.is_ident("extra_dispatch") {
                    extra_dispatch = crate::parse_utils::parse_operand_list(&nv.value)?;
                } else {
                    return Err(syn::Error::new_spanned(
                        &nv.path,
                        "enum-level `opcode_meta` only supports `extra_dispatch = [...]`",
                    ));
                }
            } else {
                return Err(syn::Error::new_spanned(
                    item,
                    "enum-level `opcode_meta` only supports `extra_dispatch = [...]`",
                ));
            }
        }
        Ok(EnumMeta { extra_dispatch })
    }
}

// ══════════════════════════════════════════════════════════════════
// 中间表示
// ══════════════════════════════════════════════════════════════════

/// 变体信息：标识符 + 字段列表 + 元数据。
struct VariantInfo {
    ident: Ident,
    fields: Vec<(Ident, Ident)>,
    meta: VariantMeta,
}

// ══════════════════════════════════════════════════════════════════
// 主入口
// ══════════════════════════════════════════════════════════════════

/// 主入口：解析 `DeriveInput` 并展开。
///
/// 接受 `&syn::DeriveInput` 以便在 `nuzo_proc` 入口直接传入，
/// 返回 `syn::Result<TokenStream>` 以支持 span-aware 错误。
pub fn expand_opcode_sync(input: &syn::DeriveInput) -> syn::Result<TokenStream> {
    // 解析枚举级属性
    let enum_meta = parse_enum_attr(&input.attrs)?;

    // 解析变体
    let variants = match &input.data {
        syn::Data::Enum(data) => &data.variants,
        _ => {
            return Err(syn::Error::new_spanned(input, "OpcodeSync only supports enums"));
        }
    };

    let mut variant_infos = Vec::with_capacity(variants.len());
    for variant in variants {
        let meta = parse_variant_attr(&variant.attrs)?;
        let fields = extract_fields(&variant.fields)?;
        variant_infos.push(VariantInfo { ident: variant.ident.clone(), fields, meta });
    }

    // 生成代码
    let ssot_macro = generate_ssot_macro(&variant_infos);
    let dispatch_list_macro = generate_dispatch_list_macro(&variant_infos, &enum_meta);
    let instruction_count = generate_instruction_count(&variant_infos, &enum_meta);
    let assertions = generate_assertions(&variant_infos, &enum_meta);

    Ok(quote! {
        #ssot_macro
        #dispatch_list_macro
        #instruction_count
        #assertions
    })
}

// ══════════════════════════════════════════════════════════════════
// 属性提取
// ══════════════════════════════════════════════════════════════════

/// 从属性列表中解析枚举级 `#[opcode_meta(...)]`。
fn parse_enum_attr(attrs: &[syn::Attribute]) -> syn::Result<EnumMeta> {
    let mut result = EnumMeta { extra_dispatch: Vec::new() };
    for attr in attrs {
        if attr.path().is_ident("opcode_meta") {
            let parsed: EnumMeta = attr.parse_args()?;
            result.extra_dispatch.extend(parsed.extra_dispatch);
        }
    }
    Ok(result)
}

/// 从属性列表中解析变体级 `#[opcode_meta(...)]`。
fn parse_variant_attr(attrs: &[syn::Attribute]) -> syn::Result<VariantMeta> {
    let mut result = VariantMeta::default();
    for attr in attrs {
        if attr.path().is_ident("opcode_meta") {
            let parsed: VariantMeta = attr.parse_args()?;
            if parsed.skip_ssot {
                result.skip_ssot = true;
            }
            if parsed.skip_dispatch {
                result.skip_dispatch = true;
            }
        }
    }
    Ok(result)
}

/// 从变体字段中提取 `(field_name, field_type)` 列表。
///
/// 仅支持命名字段（`{ dest: Reg }`）和单元变体（`Halt`），
/// 不支持元组变体（`Halt(Reg)`）。
fn extract_fields(fields: &syn::Fields) -> syn::Result<Vec<(Ident, Ident)>> {
    let mut result = Vec::new();
    match fields {
        syn::Fields::Named(named) => {
            for field in &named.named {
                let name = field
                    .ident
                    .clone()
                    .ok_or_else(|| syn::Error::new_spanned(field, "expected named field"))?;
                let ty = extract_single_ident(&field.ty)?;
                result.push((name, ty));
            }
        }
        syn::Fields::Unit => {}
        syn::Fields::Unnamed(_) => {
            return Err(syn::Error::new_spanned(
                fields,
                "OpcodeSync does not support tuple variants; use named fields",
            ));
        }
    }
    Ok(result)
}

/// 从类型路径中提取单段标识符（如 `Reg` → `Reg`）。
///
/// 不支持泛型类型（如 `Vec<Reg>`）或路径类型（如 `std::vec::Vec<Reg>`），
/// 因为 `with_every_instruction!` 的消费者宏只识别简单标识符。
fn extract_single_ident(ty: &syn::Type) -> syn::Result<Ident> {
    if let syn::Type::Path(tp) = ty
        && tp.path.segments.len() == 1
    {
        return Ok(tp.path.segments[0].ident.clone());
    }
    Err(syn::Error::new_spanned(
        ty,
        "expected a simple type identifier (e.g., `Reg`, `ConstIdx`); \
         generic/path types are not supported by OpcodeSync",
    ))
}

// ══════════════════════════════════════════════════════════════════
// 代码生成
// ══════════════════════════════════════════════════════════════════

/// 生成 `with_every_instruction!` 宏定义。
///
/// 格式与原手写版本完全兼容：
/// ```ignore
/// (LoadK, LoadK, dest: Reg, const_idx: ConstIdx);
/// ```
fn generate_ssot_macro(variants: &[VariantInfo]) -> TokenStream {
    let entries: Vec<TokenStream> = variants
        .iter()
        .filter(|v| !v.meta.skip_ssot)
        .map(|v| {
            let ident = &v.ident;
            let fields: Vec<TokenStream> =
                v.fields.iter().map(|(name, ty)| quote!(#name: #ty)).collect();
            if fields.is_empty() {
                quote!((#ident, #ident))
            } else {
                quote!((#ident, #ident, #(#fields),*))
            }
        })
        .collect();

    quote! {
        /// SSOT 指令注册表 — 由 `#[derive(OpcodeSync)]` 自动生成。
        ///
        /// 每行格式: `($Instr, $Opcode, $($name:ident : $type:ident),*)`
        ///
        /// 新增指令时只需在 `Instruction` 枚举上添加变体（带 `#[opcode_meta(...)]`），
        /// 本宏会自动同步，无需手动维护。
        ///
        /// # 排除规则
        /// 标注 `#[opcode_meta(skip_ssot)]` 的变体不纳入此宏（如 `Halt`、`Capture`）。
        #[macro_export]
        macro_rules! with_every_instruction {
            ($callback:ident) => {
                $callback! {
                    #(#entries;)*
                }
            };
        }
    }
}

/// 生成 `with_every_dispatch_opcode!` 宏定义。
///
/// 该宏供 `dispatch_table.rs` 通过 callback 模式调用 `define_dispatch_auto!`，
/// 避免手写 opcode 列表。
fn generate_dispatch_list_macro(variants: &[VariantInfo], enum_meta: &EnumMeta) -> TokenStream {
    let mut opcodes: Vec<Ident> =
        variants.iter().filter(|v| !v.meta.skip_dispatch).map(|v| v.ident.clone()).collect();
    opcodes.extend(enum_meta.extra_dispatch.iter().cloned());

    quote! {
        /// Dispatch opcode 列表 — 由 `#[derive(OpcodeSync)]` 自动生成。
        ///
        /// 包含 `Instruction` 枚举的所有非 `skip_dispatch` 变体，
        /// 加上枚举级 `#[opcode_meta(extra_dispatch = [...])]` 声明的额外 opcode。
        ///
        /// # 用法
        /// ```ignore
        /// macro_rules! build_dispatch_table {
        ///     ($($op:ident),* $(,)?) => {
        ///         nuzo_proc::define_dispatch_auto! { $($op),* }
        ///     };
        /// }
        /// with_every_dispatch_opcode!(build_dispatch_table);
        /// ```
        #[macro_export]
        macro_rules! with_every_dispatch_opcode {
            ($callback:ident) => {
                $callback! {
                    #(#opcodes),*
                }
            };
        }
    }
}

/// 生成 `INSTRUCTION_COUNT` 常量。
///
/// 值 = `Instruction` 变体数 + `extra_dispatch` 声明的额外 opcode 数。
/// 与 `Opcode::ALL.len()` 由 `opcode.rs` 末尾的 `_OPCODE_COUNT_CHECK` 断言保证一致。
fn generate_instruction_count(variants: &[VariantInfo], enum_meta: &EnumMeta) -> TokenStream {
    let total = variants.len() + enum_meta.extra_dispatch.len();
    quote! {
        /// 有效指令总数 — 由 `#[derive(OpcodeSync)]` 自动生成。
        ///
        /// 等于 `Instruction` 枚举变体数 + `extra_dispatch` 声明的额外 opcode 数。
        /// 与 `Opcode::ALL.len()` 由编译期断言 `_OPCODE_COUNT_CHECK` 保证一致。
        pub const INSTRUCTION_COUNT: usize = #total;
    }
}

/// 生成编译期断言。
///
/// 这些断言在 derive 宏生成的代码位置求值，
/// 确保内部一致性（防止手动修改常量）。
///
/// 注：与 `Opcode::ALL.len()` 的一致性断言保留在 `opcode.rs` 末尾，
/// 因为 `Opcode` 由 `define_opcodes!` 在 `Instruction` 之后生成，
/// derive 宏生成位置无法引用 `Opcode`。
fn generate_assertions(variants: &[VariantInfo], enum_meta: &EnumMeta) -> TokenStream {
    let variant_count = variants.len();
    let ssot_count = variants.iter().filter(|v| !v.meta.skip_ssot).count();
    let dispatch_count =
        variants.iter().filter(|v| !v.meta.skip_dispatch).count() + enum_meta.extra_dispatch.len();
    let total = variants.len() + enum_meta.extra_dispatch.len();

    quote! {
        // 编译期断言：INSTRUCTION_COUNT 与 derive 宏计算值一致（防止手动修改）
        const _OPCODE_SYNC_COUNT_ASSERT: () = {
            assert!(
                INSTRUCTION_COUNT == #total,
                "INSTRUCTION_COUNT has been manually modified; it must equal variant_count + extra_dispatch.len()"
            );
        };

        // 内部统计常量（供调试/文档使用）
        #[allow(dead_code)]
        const _OPCODE_SYNC_VARIANT_COUNT: usize = #variant_count;
        #[allow(dead_code)]
        const _OPCODE_SYNC_SSOT_COUNT: usize = #ssot_count;
        #[allow(dead_code)]
        const _OPCODE_SYNC_DISPATCH_COUNT: usize = #dispatch_count;
    }
}

// ══════════════════════════════════════════════════════════════════
// 单元测试
// ══════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;
    use syn::parse_quote;

    #[test]
    fn test_variant_meta_skip_ssot() {
        let attr: VariantMeta = syn::parse_str("skip_ssot").unwrap();
        assert!(attr.skip_ssot);
        assert!(!attr.skip_dispatch);
    }

    #[test]
    fn test_variant_meta_skip_dispatch() {
        let attr: VariantMeta = syn::parse_str("skip_dispatch").unwrap();
        assert!(!attr.skip_ssot);
        assert!(attr.skip_dispatch);
    }

    #[test]
    fn test_variant_meta_unknown_flag() {
        let result: Result<VariantMeta, _> = syn::parse_str("unknown_flag");
        assert!(result.is_err());
    }

    #[test]
    fn test_enum_meta_extra_dispatch() {
        let attr: EnumMeta =
            syn::parse_str("extra_dispatch = [GetGlobalCached, SpillLoad]").unwrap();
        assert_eq!(attr.extra_dispatch.len(), 2);
        assert_eq!(attr.extra_dispatch[0].to_string(), "GetGlobalCached");
    }

    #[test]
    fn test_expand_opcode_sync_basic() {
        let input: syn::DeriveInput = parse_quote! {
            #[opcode_meta(extra_dispatch = [GetGlobalCached])]
            enum Instruction {
                #[opcode_meta(skip_ssot)]
                Halt,
                LoadK { dest: Reg, const_idx: ConstIdx },
            }
        };
        let output = expand_opcode_sync(&input).unwrap();
        let output_str = output.to_string();

        // 验证生成了 with_every_instruction! 宏
        assert!(output_str.contains("with_every_instruction"));
        // 验证 Halt 被排除出 SSOT
        assert!(!output_str.contains("Halt, Halt"));
        // 验证 LoadK 在 SSOT 中
        assert!(output_str.contains("LoadK"));
        // 验证生成了 with_every_dispatch_opcode! 宏
        assert!(output_str.contains("with_every_dispatch_opcode"));
        // 验证 extra_dispatch 被包含
        assert!(output_str.contains("GetGlobalCached"));
        // 验证生成了 INSTRUCTION_COUNT
        assert!(output_str.contains("INSTRUCTION_COUNT"));
    }

    #[test]
    fn test_expand_rejects_struct() {
        let input: syn::DeriveInput = parse_quote! {
            struct Foo { x: u32 }
        };
        let result = expand_opcode_sync(&input);
        assert!(result.is_err());
    }

    #[test]
    fn test_expand_rejects_tuple_variant() {
        let input: syn::DeriveInput = parse_quote! {
            enum Foo {
                Bar(u32),
            }
        };
        let result = expand_opcode_sync(&input);
        assert!(result.is_err());
    }
}
