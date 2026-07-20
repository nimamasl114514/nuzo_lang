//! Nuzo 生态集成
//!
//! 提供 crate 路径发现（处理重命名）、标准导入生成等工具。
//!
//! # 核心能力
//!
//! - **crate 路径发现**：自动检测用户是否在 `Cargo.toml` 中重命名了 `nuzo` 或其子 crate，
//!   确保生成的代码始终引用正确的 crate 路径。
//! - **标准导入生成**：通过 [`NuzoModule`] 枚举声明所需模块，
//!   [`generate_nuzo_imports`] 自动生成对应的 `use` 语句。
//!
//! # 示例
//!
//! ```ignore
//! use nuzo_proc_core::nuzo::{nuzo_crate_path, generate_nuzo_imports, NuzoModule};
//!
//! let path = nuzo_crate_path();
//! let imports = generate_nuzo_imports(&[NuzoModule::Values, NuzoModule::Bytecode]);
//! ```

use proc_macro2;
use quote::quote;
use std::fmt;
use syn;

// ---------------------------------------------------------------------------
// 内联 crate 发现（替代 proc-macro-crate 依赖）
// ---------------------------------------------------------------------------

/// 在 proc-macro 编译期发现的 crate 信息。
#[derive(Debug, Clone)]
enum FoundCrate {
    /// 当前编译的 crate 就是目标 crate 自身。
    Itself,
    /// 目标 crate 以给定名称被发现（可能被用户重命名）。
    Name(String),
}

/// 在编译期查找指定 crate 的实际引用名称。
///
/// 实现策略（零外部依赖）：
/// 1. 读取 `CARGO_MANIFEST_DIR/Cargo.toml`，解析 `[dependencies]` 表，
///    检查目标 crate 是否被重命名（`package = "original_name"` 语法）。
/// 2. 若当前 crate 的 `CARGO_MANIFEST_DIR` 与目标 crate 名称匹配，
///    返回 `FoundCrate::Itself`。
/// 3. 若以上均失败，返回 `Err`，调用方回退到默认 crate 名。
fn find_crate(crate_name: &str) -> Result<FoundCrate, FindCrateError> {
    let manifest_dir =
        std::env::var("CARGO_MANIFEST_DIR").map_err(|_| FindCrateError::NoManifestDir)?;
    let manifest_path = std::path::Path::new(&manifest_dir).join("Cargo.toml");
    let content = std::fs::read_to_string(&manifest_path).map_err(|_| FindCrateError::IoError)?;

    // 检查当前 crate 是否就是目标 crate 自身
    if is_crate_itself(&content, crate_name) {
        return Ok(FoundCrate::Itself);
    }

    // 在 [dependencies] 中查找重命名
    if let Some(renamed) = find_renamed_dep(&content, crate_name) {
        return Ok(FoundCrate::Name(renamed));
    }

    // 未重命名，使用原始 crate 名
    Ok(FoundCrate::Name(crate_name.to_owned()))
}

/// 检查当前 Cargo.toml 的 `[package] name` 是否与目标 crate 名匹配。
fn is_crate_itself(toml_content: &str, crate_name: &str) -> bool {
    // 简易解析：查找 [package] 段下的 name 字段
    let mut in_package = false;
    for line in toml_content.lines() {
        let trimmed = line.trim();
        if trimmed == "[package]" {
            in_package = true;
            continue;
        }
        if trimmed.starts_with('[') {
            in_package = false;
            continue;
        }
        if in_package
            && let Some(name_val) = trimmed.strip_prefix("name")
            && let Some(name_val) = name_val.trim_start().strip_prefix('=')
        {
            let name_val = name_val.trim().trim_matches('"').trim();
            if name_val == crate_name {
                return true;
            }
        }
    }
    false
}

/// 在 `[dependencies]` 中查找被重命名的依赖。
///
/// TOML 重命名语法：`renamed_name = { package = "original_name", ... }`
fn find_renamed_dep(toml_content: &str, original_name: &str) -> Option<String> {
    let mut in_deps = false;
    for line in toml_content.lines() {
        let trimmed = line.trim();
        if trimmed == "[dependencies]" {
            in_deps = true;
            continue;
        }
        if trimmed.starts_with('[') {
            in_deps = false;
            continue;
        }
        if in_deps {
            // 情况 1：直接依赖 `original_name = ...`（未重命名）
            // 情况 2：重命名 `new_name = { package = "original_name", ... }`
            if let Some(eq_pos) = trimmed.find('=') {
                let key = trimmed[..eq_pos].trim();
                let value = trimmed[eq_pos + 1..].trim();

                // 检查是否是 table 形式且包含 package = "original_name"
                if value.contains("package") {
                    // 提取 package 值
                    if let Some(pkg_name) = extract_toml_string_field(value, "package")
                        && pkg_name == original_name
                        && key != original_name
                    {
                        return Some(key.to_owned());
                    }
                }
            }
        }
    }
    None
}

/// 从 TOML 内联 table 字符串中提取指定字段的值。
///
/// 输入形如 `{ version = "1", package = "foo" }`，提取 `package` 得到 `"foo"`。
fn extract_toml_string_field(table: &str, field: &str) -> Option<String> {
    let pattern = format!("{} =", field);
    if let Some(pos) = table.find(&pattern) {
        let after = &table[pos + pattern.len()..];
        let after = after.trim_start();
        if let Some(val) = after.strip_prefix('=') {
            let val = val.trim().trim_start_matches('"');
            if let Some(end) = val.find('"') {
                return Some(val[..end].to_owned());
            }
        }
        // 也处理 `field = "value"` 格式
        let val = after.trim_start_matches('=').trim().trim_start_matches('"');
        if let Some(end) = val.find('"') {
            return Some(val[..end].to_owned());
        }
    }
    None
}

/// crate 查找失败原因。
#[derive(Debug)]
enum FindCrateError {
    NoManifestDir,
    IoError,
}

// ---------------------------------------------------------------------------
// NuzoModule
// ---------------------------------------------------------------------------

/// Nuzo 生态子模块标识。
///
/// 每个变体对应一个子 crate（如 `nuzo_values`）或统一 crate 下的子模块（如 `nuzo::values`）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NuzoModule {
    Values,
    Bytecode,
    Vm,
    Compiler,
    Frontend,
    Error,
    Helpers,
    Core,
}

impl NuzoModule {
    fn submodule_name(self) -> &'static str {
        match self {
            Self::Values => "values",
            Self::Bytecode => "bytecode",
            Self::Vm => "vm",
            Self::Compiler => "compiler",
            Self::Frontend => "frontend",
            Self::Error => "error",
            Self::Helpers => "helpers",
            Self::Core => "core",
        }
    }

    fn standalone_crate_name(self) -> &'static str {
        match self {
            Self::Values => "nuzo_values",
            Self::Bytecode => "nuzo_bytecode",
            Self::Vm => "nuzo_vm",
            Self::Compiler => "nuzo_compiler",
            Self::Frontend => "nuzo_frontend",
            Self::Error => "nuzo_error",
            Self::Helpers => "nuzo_helpers",
            Self::Core => "nuzo_core",
        }
    }

    fn default_imports(self) -> &'static [&'static str] {
        match self {
            Self::Values => &["Value"],
            Self::Bytecode => &["Chunk", "Opcode"],
            Self::Vm => &["VM"],
            Self::Compiler => &["Compiler"],
            Self::Frontend => &["Token", "Lexer", "Parser"],
            Self::Error => &["NuzoError"],
            Self::Helpers => &["BuiltinFn"],
            Self::Core => &["SourceLocation"],
        }
    }
}

impl fmt::Display for NuzoModule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.submodule_name())
    }
}

// ---------------------------------------------------------------------------
// crate 路径发现
// ---------------------------------------------------------------------------

/// 获取 `nuzo` crate 的路径。
///
/// 如果用户在 `Cargo.toml` 中将 `nuzo` 重命名为 `my_nuzo`，则返回 `my_nuzo`。
/// 如果在自身 crate 内部使用，返回 `crate`。
/// 如果完全找不到，回退到 `nuzo`。
pub fn nuzo_crate_path() -> syn::Path {
    match find_crate("nuzo") {
        Ok(FoundCrate::Itself) => syn::parse_quote!(crate),
        Ok(FoundCrate::Name(name)) => {
            let ident = proc_macro2::Ident::new(&name, proc_macro2::Span::call_site());
            syn::parse_quote!(#ident)
        }
        Err(_) => syn::parse_quote!(nuzo),
    }
}

/// 获取 `nuzo_values` 子 crate / 子模块路径。
///
/// 优先尝试 `nuzo::values`，失败则回退到 `nuzo_values` 独立 crate。
pub fn nuzo_value_path() -> syn::Path {
    sub_crate_path(NuzoModule::Values)
}

/// 获取 `nuzo_bytecode` 子 crate / 子模块路径。
pub fn nuzo_bytecode_path() -> syn::Path {
    sub_crate_path(NuzoModule::Bytecode)
}

/// 获取 `nuzo_vm` 子 crate / 子模块路径。
pub fn nuzo_vm_path() -> syn::Path {
    sub_crate_path(NuzoModule::Vm)
}

/// 获取 `nuzo_compiler` 子 crate / 子模块路径。
pub fn nuzo_compiler_path() -> syn::Path {
    sub_crate_path(NuzoModule::Compiler)
}

/// 获取 `nuzo_frontend` 子 crate / 子模块路径。
pub fn nuzo_frontend_path() -> syn::Path {
    sub_crate_path(NuzoModule::Frontend)
}

/// 获取 `nuzo_error` 子 crate / 子模块路径。
pub fn nuzo_error_path() -> syn::Path {
    sub_crate_path(NuzoModule::Error)
}

/// 获取 `nuzo_helpers` 子 crate / 子模块路径。
pub fn nuzo_helpers_path() -> syn::Path {
    sub_crate_path(NuzoModule::Helpers)
}

/// 获取 `nuzo_core` 子 crate / 子模块路径。
pub fn nuzo_core_path() -> syn::Path {
    sub_crate_path(NuzoModule::Core)
}

fn sub_crate_path(module: NuzoModule) -> syn::Path {
    match find_crate("nuzo") {
        Ok(_) => {
            let base = nuzo_crate_path();
            make_path_from_base(&base, module.submodule_name())
        }
        Err(_) => match find_crate(module.standalone_crate_name()) {
            Ok(FoundCrate::Itself) => syn::parse_quote!(crate),
            Ok(FoundCrate::Name(name)) => {
                let ident = proc_macro2::Ident::new(&name, proc_macro2::Span::call_site());
                syn::parse_quote!(#ident)
            }
            Err(_) => make_path(&[module.standalone_crate_name()]),
        },
    }
}

fn make_path_from_base(base: &syn::Path, segment: &str) -> syn::Path {
    let mut segments = base.segments.clone();
    let ident = proc_macro2::Ident::new(segment, proc_macro2::Span::call_site());
    segments.push(syn::PathSegment::from(ident));
    syn::Path { leading_colon: base.leading_colon, segments }
}

// ---------------------------------------------------------------------------
// 导入生成
// ---------------------------------------------------------------------------

/// 根据指定的模块列表生成标准 Nuzo 类型导入语句。
///
/// # 示例
///
/// ```ignore
/// let tokens = generate_nuzo_imports(&[NuzoModule::Values, NuzoModule::Bytecode]);
/// // 生成:
/// // use nuzo::values::Value;
/// // use nuzo::bytecode::{Chunk, Opcode};
/// ```
pub fn generate_nuzo_imports(modules: &[NuzoModule]) -> proc_macro2::TokenStream {
    modules
        .iter()
        .map(|&module| {
            let path = sub_crate_path(module);
            let imports = module.default_imports();
            if imports.len() == 1 {
                let name = proc_macro2::Ident::new(imports[0], proc_macro2::Span::call_site());
                quote! { use #path::#name; }
            } else {
                let names: Vec<_> = imports
                    .iter()
                    .map(|&n| proc_macro2::Ident::new(n, proc_macro2::Span::call_site()))
                    .collect();
                quote! { use #path::{#(#names),*}; }
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// 辅助函数
// ---------------------------------------------------------------------------

/// 从字符串切片构建 `syn::Path`。
///
/// ```ignore
/// let path = make_path(&["nuzo", "values", "Value"]);
/// // 等价于 nuzo::values::Value
/// ```
///
/// # Panic
///
/// 当 `segments` 为空时 panic。这是 proc-macro 内部 API，
/// 调用方必须保证至少传入一个段。当前生产调用方
/// （`sub_crate_path`）始终传入单元素切片，不会触发 panic。
/// 测试 `make_path_empty_panics` 显式验证此行为。
pub fn make_path(segments: &[&str]) -> syn::Path {
    if segments.is_empty() {
        panic!(
            "make_path: at least one segment required. \
             Caller must ensure non-empty segments; this is a proc-macro invariant."
        );
    }
    let segments = segments
        .iter()
        .map(|&s| {
            let ident = proc_macro2::Ident::new(s, proc_macro2::Span::call_site());
            syn::PathSegment::from(ident)
        })
        .collect();
    syn::Path { leading_colon: None, segments }
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn make_path_single_segment() {
        let path = make_path(&["nuzo"]);
        assert_eq!(path.segments.len(), 1);
        assert_eq!(path.segments[0].ident.to_string(), "nuzo");
        assert!(path.leading_colon.is_none());
    }

    #[test]
    fn make_path_multi_segment() {
        let path = make_path(&["nuzo", "values", "Value"]);
        assert_eq!(path.segments.len(), 3);
        assert_eq!(path.segments[0].ident.to_string(), "nuzo");
        assert_eq!(path.segments[1].ident.to_string(), "values");
        assert_eq!(path.segments[2].ident.to_string(), "Value");
    }

    #[test]
    #[should_panic(expected = "at least one segment required")]
    fn make_path_empty_panics() {
        let _ = make_path(&[]);
    }

    #[test]
    fn nuzo_module_submodule_names() {
        assert_eq!(NuzoModule::Values.submodule_name(), "values");
        assert_eq!(NuzoModule::Bytecode.submodule_name(), "bytecode");
        assert_eq!(NuzoModule::Vm.submodule_name(), "vm");
        assert_eq!(NuzoModule::Compiler.submodule_name(), "compiler");
        assert_eq!(NuzoModule::Frontend.submodule_name(), "frontend");
        assert_eq!(NuzoModule::Error.submodule_name(), "error");
        assert_eq!(NuzoModule::Helpers.submodule_name(), "helpers");
        assert_eq!(NuzoModule::Core.submodule_name(), "core");
    }

    #[test]
    fn nuzo_module_standalone_crate_names() {
        assert_eq!(NuzoModule::Values.standalone_crate_name(), "nuzo_values");
        assert_eq!(NuzoModule::Bytecode.standalone_crate_name(), "nuzo_bytecode");
        assert_eq!(NuzoModule::Vm.standalone_crate_name(), "nuzo_vm");
        assert_eq!(NuzoModule::Compiler.standalone_crate_name(), "nuzo_compiler");
        assert_eq!(NuzoModule::Frontend.standalone_crate_name(), "nuzo_frontend");
        assert_eq!(NuzoModule::Error.standalone_crate_name(), "nuzo_error");
        assert_eq!(NuzoModule::Helpers.standalone_crate_name(), "nuzo_helpers");
        assert_eq!(NuzoModule::Core.standalone_crate_name(), "nuzo_core");
    }

    #[test]
    fn nuzo_module_display() {
        assert_eq!(format!("{}", NuzoModule::Values), "values");
        assert_eq!(format!("{}", NuzoModule::Bytecode), "bytecode");
    }

    #[test]
    fn nuzo_module_default_imports() {
        assert_eq!(NuzoModule::Values.default_imports(), &["Value"]);
        assert_eq!(NuzoModule::Bytecode.default_imports(), &["Chunk", "Opcode"]);
        assert_eq!(NuzoModule::Vm.default_imports(), &["VM"]);
        assert_eq!(NuzoModule::Compiler.default_imports(), &["Compiler"]);
        assert_eq!(NuzoModule::Frontend.default_imports(), &["Token", "Lexer", "Parser"]);
        assert_eq!(NuzoModule::Error.default_imports(), &["NuzoError"]);
        assert_eq!(NuzoModule::Helpers.default_imports(), &["BuiltinFn"]);
        assert_eq!(NuzoModule::Core.default_imports(), &["SourceLocation"]);
    }

    #[test]
    fn make_path_from_base_appends_segment() {
        let base = make_path(&["nuzo"]);
        let extended = make_path_from_base(&base, "values");
        assert_eq!(extended.segments.len(), 2);
        assert_eq!(extended.segments[0].ident.to_string(), "nuzo");
        assert_eq!(extended.segments[1].ident.to_string(), "values");
    }

    #[test]
    fn make_path_from_base_preserves_leading_colon() {
        let base: syn::Path = syn::parse_quote!(::nuzo);
        let extended = make_path_from_base(&base, "values");
        assert!(extended.leading_colon.is_some());
        assert_eq!(extended.segments.len(), 2);
    }

    #[test]
    fn generate_nuzo_imports_output() {
        let tokens = generate_nuzo_imports(&[NuzoModule::Values, NuzoModule::Bytecode]);
        let code = tokens.to_string();
        assert!(code.contains("Value"));
        assert!(code.contains("Chunk"));
        assert!(code.contains("Opcode"));
    }

    #[test]
    fn generate_nuzo_imports_single_import_no_braces() {
        let tokens = generate_nuzo_imports(&[NuzoModule::Values]);
        let code = tokens.to_string();
        assert!(code.contains("Value"));
        assert!(!code.contains('{'));
    }

    #[test]
    fn generate_nuzo_imports_multi_import_has_braces() {
        let tokens = generate_nuzo_imports(&[NuzoModule::Bytecode]);
        let code = tokens.to_string();
        assert!(code.contains("Chunk"));
        assert!(code.contains("Opcode"));
        assert!(code.contains('{'));
    }

    #[test]
    fn generate_nuzo_imports_empty() {
        let tokens = generate_nuzo_imports(&[]);
        assert!(tokens.is_empty());
    }

    #[test]
    fn nuzo_crate_path_fallback() {
        let path = nuzo_crate_path();
        assert_eq!(path.segments.len(), 1);
        // 在 workspace 上下文中，nuzo crate 可发现，返回 "nuzo"
        assert_eq!(path.segments[0].ident.to_string(), "nuzo");
    }

    #[test]
    fn sub_crate_path_fallback_to_standalone() {
        let path = nuzo_value_path();
        // 在 workspace 上下文中，nuzo crate 可发现，走 nuzo::values 路径
        // 否则回退到 nuzo_values 独立 crate
        if path.segments.len() == 2 {
            assert_eq!(path.segments[0].ident.to_string(), "nuzo");
            assert_eq!(path.segments[1].ident.to_string(), "values");
        } else {
            assert_eq!(path.segments.len(), 1);
            assert_eq!(path.segments[0].ident.to_string(), "nuzo_values");
        }
    }

    #[test]
    fn all_sub_crate_paths_fallback() {
        // 在 workspace 上下文中，nuzo crate 可发现，所有子路径走 nuzo::module 格式
        // 否则回退到 nuzo_module 独立 crate 格式
        let pairs: Vec<(NuzoModule, &str)> = vec![
            (NuzoModule::Values, "values"),
            (NuzoModule::Bytecode, "bytecode"),
            (NuzoModule::Vm, "vm"),
            (NuzoModule::Compiler, "compiler"),
            (NuzoModule::Frontend, "frontend"),
            (NuzoModule::Error, "error"),
            (NuzoModule::Helpers, "helpers"),
            (NuzoModule::Core, "core"),
        ];
        for (module, sub_name) in pairs {
            let path = sub_crate_path(module);
            if path.segments.len() == 2 {
                assert_eq!(path.segments[0].ident.to_string(), "nuzo");
                assert_eq!(path.segments[1].ident.to_string(), sub_name);
            } else {
                assert_eq!(path.segments.len(), 1);
                assert!(path.segments[0].ident.to_string().starts_with("nuzo_"));
            }
        }
    }

    #[test]
    fn nuzo_module_exhaustive_match() {
        let all: Vec<NuzoModule> = vec![
            NuzoModule::Values,
            NuzoModule::Bytecode,
            NuzoModule::Vm,
            NuzoModule::Compiler,
            NuzoModule::Frontend,
            NuzoModule::Error,
            NuzoModule::Helpers,
            NuzoModule::Core,
        ];
        assert_eq!(all.len(), 8);
        for module in all {
            assert!(!module.submodule_name().is_empty());
            assert!(module.standalone_crate_name().starts_with("nuzo_"));
            assert!(!module.default_imports().is_empty());
        }
    }
}
