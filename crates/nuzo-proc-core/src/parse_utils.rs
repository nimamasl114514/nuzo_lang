//! 共享解析工具 — 消除各宏间的重复解析逻辑
//!
//! 提供 `define_opcodes!` / `nuzo_test` / `FromMeta` / `MatchSync` 等宏
//! 共用的字面量提取函数。所有函数返回 `syn::Result<T>` 以支持 span-aware 错误。

use syn::{Expr, Lit};

// ══════════════════════════════════════════════════════════════════
// 字面量解析
// ══════════════════════════════════════════════════════════════════

/// 从表达式中解析整数字面量（泛型版本）。
///
/// 支持普通整数字面量和负数（一元减号表达式，如 `-42`）。
///
/// # 参数
/// - `expr`: 待解析的表达式
/// - `field_name`: 字段名（用于错误信息）
/// - `range_hint`: 范围提示（如 `" (0..=255)"`）
///
/// # 示例
/// ```
/// use syn::{Expr, parse_quote};
/// use nuzo_proc_core::parse_utils::parse_int_lit;
///
/// # fn main() -> syn::Result<()> {
/// let expr: Expr = parse_quote!(42);
/// let val: u8 = parse_int_lit(&expr, "code", " (0..=255)")?;
/// assert_eq!(val, 42u8);
/// # Ok(())
/// # }
/// ```
pub fn parse_int_lit<T: std::str::FromStr>(
    expr: &Expr,
    field_name: &str,
    range_hint: &str,
) -> syn::Result<T>
where
    <T as std::str::FromStr>::Err: std::fmt::Display,
{
    match expr {
        Expr::Lit(lit) => match &lit.lit {
            Lit::Int(i) => i.base10_parse::<T>(),
            other => Err(crate::diag::SpannedError::new_spanned(
                other,
                format!("`{field_name}` must be an integer literal{range_hint}"),
            )
            .into_inner()),
        },
        // 支持负数：-42 → Expr::Unary { op: Minus, expr: Lit(42) }
        Expr::Unary(unary) if matches!(unary.op, syn::UnOp::Neg(_)) => {
            let inner = &unary.expr;
            match inner.as_ref() {
                Expr::Lit(lit) => match &lit.lit {
                    Lit::Int(i) => {
                        let s = format!("-{}", i.base10_digits());
                        s.parse::<T>().map_err(|_| {
                            crate::diag::SpannedError::new_spanned(
                                expr,
                                format!("`{field_name}`: invalid negative integer literal"),
                            )
                            .into_inner()
                        })
                    }
                    other => Err(crate::diag::SpannedError::new_spanned(
                        other,
                        format!("`{field_name}` must be an integer literal{range_hint}"),
                    )
                    .into_inner()),
                },
                other => Err(crate::diag::SpannedError::new_spanned(
                    other,
                    format!("`{field_name}` must be an integer literal"),
                )
                .into_inner()),
            }
        }
        other => Err(crate::diag::SpannedError::new_spanned(
            other,
            format!("`{field_name}` must be an integer literal"),
        )
        .into_inner()),
    }
}

/// 从表达式中解析字符串字面量。
pub fn parse_string_lit(expr: &Expr) -> syn::Result<String> {
    match expr {
        Expr::Lit(lit) => match &lit.lit {
            Lit::Str(s) => Ok(s.value()),
            other => {
                Err(crate::diag::SpannedError::new_spanned(other, "expected a string literal")
                    .into_inner())
            }
        },
        other => {
            Err(crate::diag::SpannedError::new_spanned(other, "expected a string literal")
                .into_inner())
        }
    }
}

/// 从表达式中解析布尔字面量。
pub fn parse_bool_lit(expr: &Expr) -> syn::Result<bool> {
    match expr {
        Expr::Lit(lit) => match &lit.lit {
            Lit::Bool(b) => Ok(b.value()),
            other => {
                Err(crate::diag::SpannedError::new_spanned(other, "expected a boolean literal")
                    .into_inner())
            }
        },
        other => Err(crate::diag::SpannedError::new_spanned(other, "expected a boolean literal")
            .into_inner()),
    }
}

/// 从表达式中解析浮点数字面量（f64）。
pub fn parse_f64_lit(expr: &Expr) -> syn::Result<f64> {
    match expr {
        Expr::Lit(lit) => match &lit.lit {
            Lit::Float(f) => f.base10_parse::<f64>(),
            other => Err(crate::diag::SpannedError::new_spanned(other, "expected a float literal")
                .into_inner()),
        },
        other => {
            Err(crate::diag::SpannedError::new_spanned(other, "expected a float literal")
                .into_inner())
        }
    }
}

// ══════════════════════════════════════════════════════════════════
// 复合类型解析
// ══════════════════════════════════════════════════════════════════

/// 从表达式中提取字符串数组字面量 `["a", "b"]`。
pub fn extract_string_array(expr: &Expr) -> syn::Result<Vec<syn::LitStr>> {
    match expr {
        syn::Expr::Array(arr) => {
            let mut strs = Vec::with_capacity(arr.elems.len());
            for elem in &arr.elems {
                match elem {
                    syn::Expr::Lit(lit) => match &lit.lit {
                        syn::Lit::Str(s) => strs.push(s.clone()),
                        other => {
                            return Err(syn::Error::new_spanned(
                                other,
                                "expected string literal inside array",
                            ));
                        }
                    },
                    other => return Err(syn::Error::new_spanned(other, "expected string literal")),
                }
            }
            Ok(strs)
        }
        other => Err(syn::Error::new_spanned(other, "expected array of strings `[\"...\"]`")),
    }
}

/// 从表达式中解析操作数列表 `[Reg, Const, ...]`。
pub fn parse_operand_list(expr: &Expr) -> syn::Result<Vec<syn::Ident>> {
    match expr {
        syn::Expr::Array(arr) => {
            let mut opers = Vec::with_capacity(arr.elems.len());
            for elem in &arr.elems {
                match elem {
                    syn::Expr::Path(p) => {
                        if p.path.segments.len() == 1 {
                            opers.push(p.path.segments[0].ident.clone());
                        } else {
                            return Err(crate::diag::SpannedError::new_spanned(
                                elem,
                                "operand kind must be a single identifier (e.g., `Reg`, `Const`)",
                            )
                            .into_inner());
                        }
                    }
                    other => {
                        return Err(crate::diag::SpannedError::new_spanned(
                            other,
                            "operand kind must be an identifier",
                        )
                        .into_inner());
                    }
                }
            }
            Ok(opers)
        }
        other => Err(crate::diag::SpannedError::new_spanned(
            other,
            "`operands` must be an array of identifiers, e.g., `[Reg, Const]`",
        )
        .into_inner()),
    }
}

/// 从表达式中解析标识符路径（单段）。
pub fn parse_ident_path(expr: &Expr) -> syn::Result<syn::Path> {
    match expr {
        syn::Expr::Path(p) => {
            if p.path.segments.len() == 1 {
                Ok(p.path.clone())
            } else {
                Err(crate::diag::SpannedError::new_spanned(
                    expr,
                    "must be a simple identifier (e.g., `Custom`, `BinaryArithmetic`)",
                )
                .into_inner())
            }
        }
        other => {
            Err(crate::diag::SpannedError::new_spanned(other, "must be an identifier").into_inner())
        }
    }
}

// ══════════════════════════════════════════════════════════════════
// CamelCase ⇄ snake_case 转换
// ══════════════════════════════════════════════════════════════════

/// 将 CamelCase 标识符转换为 snake_case（通用版，用于 handler 推导等场景）。
///
/// | 输入 | 输出 |
/// |------|------|
/// | `LoadK` | `load_k` |
/// | `GetCaptured` | `get_captured` |
/// | `JSON` | `json` |
pub fn camel_to_snake(s: &str) -> String {
    let mut result = String::with_capacity(s.len() + s.len() / 4);
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

/// 将 CamelCase 标识符转换为带前缀的 snake_case（opcode handler 命名约定）。
///
/// | 输入 | 输出 |
/// |------|------|
/// | `LoadK` | `_op_loadk` |
/// | `Add` | `_op_add` |
pub fn camel_to_snake_op(ident: &str) -> String {
    format!("_op_{}", ident.to_lowercase())
}

// ══════════════════════════════════════════════════════════════════
// OperandKind 字节宽度映射
// ══════════════════════════════════════════════════════════════════

/// OperandKind 标识符到编译期字节宽度的映射表。
///
/// 映射关系必须与 `$crate::OperandKind::byte_size()` 保持一致。
pub fn operand_byte_size(kind_str: &str) -> syn::Result<usize> {
    match kind_str {
        "Reg" | "Const" | "Offset" | "U16" | "CaptureIdx" => Ok(2),
        "U8" => Ok(1),
        "U32" => Ok(4),
        "None" | "Nil" => Ok(0),
        "Prop" => Ok(2), // 属性访问指令的内部幽灵类型
        _ => Err(syn::Error::new(
            proc_macro2::Span::call_site(),
            format!(
                "unknown operand type: '{}'. Valid types: Reg, Const, Offset, U8, U16, U32, CaptureIdx, Prop, Nil, None",
                kind_str
            ),
        )),
    }
}

// ══════════════════════════════════════════════════════════════════
// Tests
// ══════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;
    use syn::parse_quote;

    // ── parse_int_lit ─────────────────────────────────────

    #[test]
    fn parse_u8_lit_ok() {
        let expr: Expr = parse_quote!(42);
        assert_eq!(parse_int_lit::<u8>(&expr, "code", " (0..=255)").unwrap(), 42u8);
    }

    #[test]
    fn parse_usize_lit_ok() {
        let expr: Expr = parse_quote!(1000);
        assert_eq!(parse_int_lit::<usize>(&expr, "size", "").unwrap(), 1000);
    }

    #[test]
    fn parse_i64_negative() {
        let expr: Expr = parse_quote!(-42);
        assert_eq!(parse_int_lit::<i64>(&expr, "offset", "").unwrap(), -42i64);
    }

    #[test]
    fn parse_int_rejects_float() {
        let expr: Expr = parse_quote!(2.5);
        assert!(parse_int_lit::<u8>(&expr, "code", "").is_err());
    }

    // ── parse_string_lit ───────────────────────────────────

    #[test]
    fn parse_string_lit_ok() {
        let expr: Expr = parse_quote!("hello");
        assert_eq!(parse_string_lit(&expr).unwrap(), "hello");
    }

    #[test]
    fn parse_string_lit_rejects_int() {
        let expr: Expr = parse_quote!(42);
        assert!(parse_string_lit(&expr).is_err());
    }

    // ── parse_bool_lit ────────────────────────────────────

    #[test]
    fn parse_bool_lit_true() {
        let expr: Expr = parse_quote!(true);
        assert!(parse_bool_lit(&expr).unwrap());
    }

    // ── parse_f64_lit ────────────────────────────────────

    #[test]
    fn parse_f64_lit_ok() {
        let expr: Expr = parse_quote!(2.5);
        assert!((parse_f64_lit(&expr).unwrap() - 2.5).abs() < f64::EPSILON);
    }

    // ── extract_string_array ─────────────────────────────

    #[test]
    fn extract_string_array_valid() {
        let expr: Expr = parse_quote!(["a", "b", "c"]);
        let vals = extract_string_array(&expr).unwrap();
        assert_eq!(vals.len(), 3);
        assert_eq!(vals[0].value(), "a");
    }

    #[test]
    fn extract_string_array_empty() {
        let expr: Expr = parse_quote!([]);
        assert!(extract_string_array(&expr).unwrap().is_empty());
    }

    // ── parse_operand_list ────────────────────────────────

    #[test]
    fn parse_operand_list_valid() {
        let expr: Expr = parse_quote!([Reg, Const, Offset]);
        let ops = parse_operand_list(&expr).unwrap();
        assert_eq!(ops.len(), 3);
        assert_eq!(ops[0], "Reg");
    }

    // ── camel_to_snake ───────────────────────────────────

    #[test]
    fn camel_to_snake_basic() {
        assert_eq!(camel_to_snake("LoadK"), "load_k");
        assert_eq!(camel_to_snake("ArrayNew"), "array_new");
        assert_eq!(camel_to_snake("GetCaptured"), "get_captured");
        assert_eq!(camel_to_snake("JSON"), "json");
        assert_eq!(camel_to_snake("URL"), "url");
    }

    // ── camel_to_snake_op ────────────────────────────────

    #[test]
    fn camel_to_snake_op_basic() {
        assert_eq!(camel_to_snake_op("LoadK"), "_op_loadk");
        assert_eq!(camel_to_snake_op("Add"), "_op_add");
        assert_eq!(camel_to_snake_op("GetCaptured"), "_op_getcaptured");
    }

    // ── operand_byte_size ────────────────────────────────

    #[test]
    fn operand_byte_size_known_types() {
        assert_eq!(operand_byte_size("Reg").unwrap(), 2);
        assert_eq!(operand_byte_size("U8").unwrap(), 1);
        assert_eq!(operand_byte_size("U32").unwrap(), 4);
        assert_eq!(operand_byte_size("None").unwrap(), 0);
        assert_eq!(operand_byte_size("Prop").unwrap(), 2);
    }

    #[test]
    fn operand_byte_size_unknown_type() {
        assert!(operand_byte_size("Unknown").is_err());
    }
}
