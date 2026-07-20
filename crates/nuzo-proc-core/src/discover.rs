//! `test_bind::discover!` 宏核心逻辑
//!
//! 生成模块路径发现注释和编译期可见性标记。
//! 由于 proc-macro 无法在编译时真正扫描文件系统，采用预置的模块映射表方案，
//! 通过 `use` 语句让编译器验证路径存在性。

use proc_macro2::TokenStream;
use quote::quote;
use syn::LitStr;

// ---------------------------------------------------------------------------
// 已知模块映射
// ---------------------------------------------------------------------------

/// 预设的 crate 路径 → 子模块名称映射表。
///
/// 格式：`("crate_path", &["submodule1", "submodule2", ...])`
///
/// # 为什么不改为运行时扫描 crates/ 目录？
///
/// 1. **proc-macro 上下文限制**：本表在 `expand_discover` 中读取，
///    该函数是 proc-macro，运行在编译期。`CARGO_MANIFEST_DIR` 指向
///    `nuzo-proc-core` 自身的目录，不是消费者 crate 的目录，也不一定
///    是 nuzo_lang 仓库根目录（crate 发布到 crates.io 后路径不存在）。
/// 2. **缓存失效问题**：Cargo 不知道 proc-macro 读了什么文件，扫描
///    结果不会触发 proc-macro 重新执行（除非显式 `rerun-if-changed=`，
///    但跨 crate 路径不可靠）。
/// 3. **性能可接受**：本表只在编译期被 proc-macro 读取一次，写入到
///    生成的常量中，运行期零开销。
///
/// # 维护
///
/// 新增 crate 时需同步更新本表与 `all_known_modules_in_table` 测试。
/// 测试会失败提醒开发者补齐条目，避免漏更新。
const KNOWN_MODULES: &[(&str, &[&str])] = &[
    (
        "crate::nuzo_helpers",
        &["math", "string", "io", "time", "array", "builtins", "convert", "debug", "validation"],
    ),
    ("crate::nuzo_core", &["constants", "encoding", "macros", "source_location"]),
    (
        "crate::nuzo_values",
        &[
            "value",
            "heap",
            "function",
            "context",
            "constants",
            "errors",
            "layout",
            "traits",
            "tag_registry",
            "inspector",
        ],
    ),
    ("crate::nuzo_vm", &["vm", "gc", "dispatch", "cache", "object", "frame_paging", "hints"]),
    (
        "crate::nuzo_compiler",
        &["compiler", "allocator", "expressions", "statements", "functions", "macros", "helpers"],
    ),
    ("crate::nuzo_frontend", &["lexer", "parser", "ast", "token"]),
    ("crate::nuzo_bytecode", &["opcode", "constants", "scope"]),
    ("crate::nuzo_error", &["types", "diagnostic", "classifier", "collector", "formatter"]),
    ("crate::nuzo_signal", &["signal", "bus", "slot", "types"]),
    ("crate::nuzo_testkit", &["inspector", "tracer", "baseline", "statistics", "stress_test"]),
];

// ---------------------------------------------------------------------------
// 核心函数
// ---------------------------------------------------------------------------

/// 展开 `discover!` 宏，生成模块路径发现注释
///
/// 输入: `LitStr` 路径字面量，如 `"crate::nuzo_helpers"`
/// 输出: 编译期文档常量 + `cfg(test)` 可见性标记
///
/// 由于 proc-macro 无法在编译时真正扫描文件系统，采用预置模块映射表方案。
/// 生成的 `use` 语句让编译器验证路径存在性，文档常量记录模块清单。
///
/// # 示例
///
/// ```ignore
/// let path: syn::LitStr = syn::parse_quote!("crate::nuzo_helpers");
/// let tokens = expand_discover(&path)?;
/// ```
pub fn expand_discover(path_lit: &LitStr) -> syn::Result<TokenStream> {
    let path_str = path_lit.value();

    // 先解析路径，失败则提前返回错误
    let use_path: syn::Path = syn::parse_str(&path_str)?;

    let sanitized = sanitize_path(&path_str);

    let modules = lookup_modules(&path_str);

    let doc_lines = build_doc_lines(&path_str, modules);

    // 生成常量名（如 _DISCOVER_nuzo_helpers）
    let const_name = quote::format_ident!("_DISCOVER_{}", sanitized);

    // 生成模块清单字符串（如 "nuzo_helpers: math, string, io, ..."）
    let manifest = build_manifest(&path_str, modules);

    let expanded = quote! {
        #[cfg(test)]
        #(#doc_lines)*
        #[allow(dead_code)]
        const #const_name: &str = #manifest;

        #[cfg(test)]
        #[allow(unused_imports)]
        use #use_path;
    };

    Ok(expanded)
}

// ---------------------------------------------------------------------------
// 辅助函数
// ---------------------------------------------------------------------------

/// 将路径中的 `::` 替换为 `_` 作为常量名后缀。
///
/// 例：`"crate::nuzo_helpers"` → `"nuzo_helpers"`
fn sanitize_path(path: &str) -> String {
    path.split("::").last().unwrap_or(path).to_owned()
}

/// 在预置映射表中查找指定路径的子模块列表。
fn lookup_modules(path: &str) -> Option<&[&str]> {
    // 先精确匹配
    for (crate_path, modules) in KNOWN_MODULES {
        if *crate_path == path {
            return Some(modules);
        }
    }
    // 路径未找到，返回 None 表示使用通用绑定
    None
}

/// 构建文档属性行的 TokenStream。
fn build_doc_lines(path: &str, modules: Option<&[&str]>) -> Vec<TokenStream> {
    let mut lines = Vec::new();

    let header = format!(" discover!(\"{}\") 模块清单:", path);
    lines.push(quote! { #[doc = #header] });

    lines.push(quote! { #[doc = "  (使用 `test_bind::bind!` 绑定以下模块)"] });

    if let Some(mods) = modules {
        for m in mods {
            let text = format!("  - {}", m);
            lines.push(quote! { #[doc = #text] });
        }
    } else {
        lines.push(quote! { #[doc = "  (无预置模块映射，使用通用绑定)"] });
    }

    lines
}

/// 构建模块清单字符串。
fn build_manifest(path: &str, modules: Option<&[&str]>) -> String {
    let base = sanitize_path(path);
    match modules {
        Some(mods) if !mods.is_empty() => {
            format!("{}: {}", base, mods.join(", "))
        }
        _ => {
            format!("{}: (unknown)", base)
        }
    }
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_crate_path() {
        assert_eq!(sanitize_path("crate::nuzo_helpers"), "nuzo_helpers");
        assert_eq!(sanitize_path("crate::nuzo_core"), "nuzo_core");
        assert_eq!(sanitize_path("nuzo_helpers"), "nuzo_helpers");
    }

    #[test]
    fn lookup_known_module() {
        let result = lookup_modules("crate::nuzo_helpers");
        assert!(result.is_some());
        let mods = result.unwrap();
        assert!(mods.contains(&"math"));
        assert!(mods.contains(&"string"));
        assert_eq!(mods.len(), 9);
    }

    #[test]
    fn lookup_unknown_module() {
        let result = lookup_modules("crate::nonexistent");
        assert!(result.is_none());
    }

    #[test]
    fn manifest_with_known_modules() {
        let s = build_manifest("crate::nuzo_bytecode", Some(&["opcode", "constants", "scope"]));
        assert!(s.starts_with("nuzo_bytecode:"));
        assert!(s.contains("opcode"));
        assert!(s.contains("constants"));
        assert!(s.contains("scope"));
    }

    #[test]
    fn manifest_with_unknown_modules() {
        let s = build_manifest("crate::nonexistent", None);
        assert_eq!(s, "nonexistent: (unknown)");
    }

    #[test]
    fn expand_discover_known_crate() {
        let path_lit: LitStr = syn::parse_quote!("crate::nuzo_helpers");
        let result = expand_discover(&path_lit);
        assert!(result.is_ok());
        let tokens = result.unwrap();
        let code = tokens.to_string();
        assert!(code.contains("_DISCOVER_nuzo_helpers"));
        assert!(code.contains("math"));
        assert!(code.contains("string"));
        assert!(code.contains("nuzo_helpers"));
    }

    #[test]
    fn expand_discover_unknown_crate() {
        let path_lit: LitStr = syn::parse_quote!("crate::unknown_mod");
        let result = expand_discover(&path_lit);
        assert!(result.is_ok());
        let tokens = result.unwrap();
        let code = tokens.to_string();
        assert!(code.contains("_DISCOVER_unknown_mod"));
        assert!(code.contains("unknown"));
    }

    #[test]
    fn expand_discover_generates_use_statement() {
        let path_lit: LitStr = syn::parse_quote!("crate::nuzo_core");
        let result = expand_discover(&path_lit);
        assert!(result.is_ok());
        let code = result.unwrap().to_string();
        assert!(code.contains("use crate :: nuzo_core"));
    }

    #[test]
    fn expand_discover_invalid_path_returns_error() {
        let path_lit: LitStr = syn::parse_quote!("not a valid path");
        let result = expand_discover(&path_lit);
        assert!(result.is_err());
    }

    #[test]
    fn all_known_modules_in_table() {
        let expected_crates = [
            "crate::nuzo_helpers",
            "crate::nuzo_core",
            "crate::nuzo_values",
            "crate::nuzo_vm",
            "crate::nuzo_compiler",
            "crate::nuzo_frontend",
            "crate::nuzo_bytecode",
            "crate::nuzo_error",
            "crate::nuzo_signal",
            "crate::nuzo_testkit",
        ];
        assert_eq!(KNOWN_MODULES.len(), expected_crates.len());
        for expected in expected_crates {
            assert!(
                KNOWN_MODULES.iter().any(|(crate_path, _)| *crate_path == expected),
                "missing crate in KNOWN_MODULES: {}",
                expected
            );
        }
    }
}
