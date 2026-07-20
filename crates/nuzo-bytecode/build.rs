// build.rs — Opcode 自动代码生成器
//
// 从 src/opcode.rs 中解析 Instruction 枚举和 define_opcodes! 宏调用，
// 自动生成常量数组（size, name, dispatch_kind）和测试代码到 $OUT_DIR。
//
// 这确保了新增 Opcode 时无需手动维护 size/name 测试。
// 参见 docs/opcode-auto-gen.md

use std::env;
use std::fs;
use std::path::Path;
use syn::{Attribute, Ident, Item, ItemEnum};

fn main() {
    // 读取源文件
    // build.rs 在编译期运行，错误应通过 cargo:warning= 输出友好信息后 panic。
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| {
        panic!("build.rs: CARGO_MANIFEST_DIR not set; must be invoked by cargo")
    });
    let source_path = Path::new(&manifest_dir).join("src/opcode.rs");
    let source = fs::read_to_string(&source_path).unwrap_or_else(|err| {
        println!(
            "cargo:warning=build.rs: failed to read src/opcode.rs at {}: {}",
            source_path.display(),
            err
        );
        panic!("build.rs: failed to read src/opcode.rs: {}", err)
    });

    // 解析为 AST
    let ast = syn::parse_file(&source).unwrap_or_else(|err| {
        println!("cargo:warning=build.rs: failed to parse src/opcode.rs: {}", err);
        panic!("build.rs: failed to parse src/opcode.rs: {}", err)
    });

    // 提取 Instruction 枚举和 define_opcodes! 宏调用
    let (_instruction_enum, macro_opcodes) = extract_instruction_and_opcodes(&ast);

    let instruction_count = macro_opcodes.len();

    // --- 生成常量数组 ---

    let sizes: Vec<u8> = macro_opcodes.iter().map(|o| o.size).collect();
    let names: Vec<&str> = macro_opcodes.iter().map(|o| o.name.as_str()).collect();
    let codes: Vec<u8> = macro_opcodes.iter().map(|o| o.code).collect();
    let descs: Vec<&str> = macro_opcodes.iter().map(|o| o.desc.as_str()).collect();
    let summaries: Vec<&str> = macro_opcodes.iter().map(|o| o.summary.as_str()).collect();

    // 生成 generated_constants.rs
    let consts = format!(
        r#"// @generated - DO NOT EDIT (by build.rs)

/// Opcode 字节数数组，按 Opcode::ALL 排序。
/// 与 Opcode::instruction_size() 一致，编译期由测试验证。
pub const OPCODE_SIZES: [u8; {count}] = {sizes:?};

/// Opcode 名称数组，按 Opcode::ALL 排序。
/// 与 Opcode::name() 一致，编译期由测试验证。
pub const OPCODE_NAMES: [&str; {count}] = {names:?};

/// Opcode 代码值数组，按 Opcode::ALL 排序。
pub const OPCODE_CODES: [u8; {count}] = {codes:?};

/// Opcode 描述数组，按 Opcode::ALL 排序。
/// 来自 #[opcode(desc = "...")] 属性，未声明时为空字符串。
pub const OPCODE_DESCS: [&str; {count}] = {descs:?};

/// Opcode 摘要数组，按 Opcode::ALL 排序。
/// 来自 #[opcode(summary = "...")] 属性，未声明时为空字符串。
pub const OPCODE_SUMMARIES: [&str; {count}] = {summaries:?};
"#,
        count = instruction_count,
        sizes = sizes,
        names = names,
        codes = codes,
        descs = descs,
        summaries = summaries,
    );

    // 生成 generated_tests.rs
    // 注意：这些测试引用 instruction_enum 中的变体名和 define_opcodes! 宏
    // 中定义的 size 属性值；两个源不一致即触发测试失败。
    let tests = r#"// @generated - DO NOT EDIT (by build.rs)

/// 验证 OPCODE_SIZES 与 instruction_size() 方法一致。
#[test]
fn generated_opcode_sizes_match_runtime() {
    for (i, (op, &expected)) in Opcode::ALL.iter().zip(crate::constants::OPCODE_SIZES.iter()).enumerate() {
        let actual = op.instruction_size() as u8;
        assert_eq!(
            actual, expected,
            "Opcode #{i} {:?} (code={}): instruction_size()={} but generated OPCODE_SIZES={}",
            op, crate::constants::OPCODE_CODES[i], actual, expected,
        );
    }
}

/// 验证 OPCODE_NAMES 与 Opcode::name() 一致。
#[test]
fn generated_opcode_names_match_runtime() {
    for (i, (op, &expected_name)) in Opcode::ALL.iter().zip(crate::constants::OPCODE_NAMES.iter()).enumerate() {
        let actual_name = op.name();
        assert_eq!(
            actual_name, expected_name,
            "Opcode #{i} {:?} (code={}): name()='{}' but generated OPCODE_NAMES='{}'",
            op, crate::constants::OPCODE_CODES[i], actual_name, expected_name,
        );
    }
}

/// 验证生成数组长度与 INSTRUCTION_COUNT 一致。
#[test]
fn generated_arrays_length_match() {
    assert_eq!(crate::constants::OPCODE_CODES.len(), INSTRUCTION_COUNT);
}
"#.to_string();

    // 输出到 OUT_DIR
    // OUT_DIR 由 cargo 设置；若缺失属于不正常构建环境，panic 退出。
    let out_dir = env::var("OUT_DIR")
        .unwrap_or_else(|_| panic!("build.rs: OUT_DIR not set; must be invoked by cargo"));
    let write_helper = |name: &str, content: &str| {
        let path = Path::new(&out_dir).join(name);
        fs::write(&path, content).unwrap_or_else(|err| {
            println!("cargo:warning=build.rs: failed to write {}: {}", path.display(), err);
            panic!("build.rs: failed to write {}: {}", name, err)
        });
    };
    write_helper("generated_constants.rs", &consts);
    write_helper("generated_tests.rs", &tests);

    // 生成 opcode_docs.md（GitHub Flavored Markdown 兼容的参考表）
    // 用于 justfile gen-opcode-docs recipe 拷贝到 opcode-reference.md
    let docs_md = generate_opcode_docs_md(&macro_opcodes);
    write_helper("opcode_docs.md", &docs_md);

    // 触发重建：opcode.rs 变更时必须重跑 build.rs
    println!("cargo:rerun-if-changed=src/opcode.rs");
}

/// 生成 Opcode 参考表（Markdown 格式）。
///
/// 输出 GitHub Flavored Markdown 兼容的表格，列出所有 Opcode 的
/// name / code / size / desc / summary，用于 opcode-reference.md。
/// desc 或 summary 为空时显示 `(undocumented)` 占位符。
fn generate_opcode_docs_md(infos: &[OpcodeInfo]) -> String {
    let mut md = String::new();
    md.push_str("# Opcode Reference\n\n");
    md.push_str("> 自动生成，请勿手改。由 `nuzo_bytecode/build.rs` 从 `opcode.rs` 生成。\n\n");
    md.push_str("| Name | Code | Size | Description | Summary |\n");
    md.push_str("|------|------|------|-------------|--------|\n");
    for info in infos {
        let desc = if info.desc.is_empty() { "(undocumented)" } else { &info.desc };
        let summary = if info.summary.is_empty() { "(undocumented)" } else { &info.summary };
        md.push_str(&format!(
            "| {} | {} | {} | {} | {} |\n",
            info.name, info.code, info.size, desc, summary,
        ));
    }
    md
}

/// 从 opcode.rs 中提取 Instruction 枚举和 define_opcodes! 宏调用中的 opcode 列表。
fn extract_instruction_and_opcodes(ast: &syn::File) -> (Option<&ItemEnum>, Vec<OpcodeInfo>) {
    let mut instruction_enum: Option<&ItemEnum> = None;
    let mut opcodes: Vec<OpcodeInfo> = Vec::new();

    for item in &ast.items {
        match item {
            Item::Enum(e) if e.ident == "Instruction" => {
                instruction_enum = Some(e);
            }
            Item::Macro(mac) => {
                // 匹配 nuzo_proc::define_opcodes! { ... }
                if let Some(seg) = mac.mac.path.segments.last()
                    && seg.ident == "define_opcodes"
                {
                    opcodes = parse_define_opcodes(&mac.mac.tokens);
                }
            }
            _ => {}
        }
    }

    (instruction_enum, opcodes)
}

/// Opcode 元数据
#[derive(Debug)]
struct OpcodeInfo {
    name: String,
    code: u8,
    size: u8,
    desc: String,
    summary: String,
}

/// 解析 define_opcodes! 宏调用中的属性标注和 opcode 标识符。
///
/// 输入格式:
/// ```text
/// #[opcode(code = N, size = S, operands = [...], disasm = X, dispatch = Y, desc = "...", summary = "...")]
/// OpcodeName,
/// ...
/// ```
fn parse_define_opcodes(tokens: &proc_macro2::TokenStream) -> Vec<OpcodeInfo> {
    let mut opcodes = Vec::new();

    // 使用 syn::parse2 解析
    // 格式为: 零个或多个 attributes, 然后一个 Ident, 逗号, 重复
    if let Ok(parsed) = syn::parse2::<OpcodesMacroBody>(tokens.clone()) {
        for entry in parsed.entries {
            let mut code: u8 = 0;
            let mut size: u8 = 1;
            let mut desc: String = String::new();
            let mut summary: String = String::new();

            for attr in &entry.attrs {
                if attr.path().is_ident("opcode") {
                    // 解析 #[opcode(code = N, size = S, operands = [...], disasm = X, dispatch = D, ...)]
                    if let Ok(nvps) = attr.parse_args::<OpcodeAttrs>() {
                        for nvp in nvps.pairs {
                            if nvp.name == "code" {
                                code = parse_u8_lit(&nvp.value);
                            } else if nvp.name == "size" {
                                size = parse_u8_lit(&nvp.value);
                            } else if nvp.name == "desc" {
                                desc = nvp.value.clone();
                            } else if nvp.name == "summary" {
                                summary = nvp.value.clone();
                            }
                        }
                    }
                }
            }

            opcodes.push(OpcodeInfo { name: entry.name.to_string(), code, size, desc, summary });
        }
    }

    // 保留声明顺序 (Opcode::ALL 按宏中的声明顺序排列，非 code 升序)
    opcodes
}

/// 解析 u8 字面量 (如 `5`, `28`)
fn parse_u8_lit(s: &str) -> u8 {
    s.parse().unwrap_or_else(|_| {
        println!("cargo:warning=build.rs: invalid u8 literal in opcode attribute: '{}'", s);
        panic!("build.rs: invalid u8 literal in opcode attribute: '{}'", s)
    })
}

// ========================================================================
// syn 解析辅助结构体
// ========================================================================

use syn::parse::{Parse, ParseStream};

/// define_opcodes! 宏体的内容: 重复的 `#[attr] Ident,`
struct OpcodesMacroBody {
    entries: Vec<OpcodesMacroEntry>,
}

struct OpcodesMacroEntry {
    attrs: Vec<Attribute>,
    name: Ident,
}

impl Parse for OpcodesMacroBody {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut entries = Vec::new();
        while !input.is_empty() {
            let attrs = input.call(Attribute::parse_outer)?;
            let name: Ident = input.parse()?;
            let _comma: syn::Token![,] = input.parse()?;
            entries.push(OpcodesMacroEntry { attrs, name });
        }
        Ok(OpcodesMacroBody { entries })
    }
}

/// #[opcode(...)] 内部的键值对列表: `code = 0, size = 5, ...`
struct OpcodeAttrs {
    pairs: Vec<KeyValuePair>,
}

struct KeyValuePair {
    name: String,
    value: String,
}

impl Parse for OpcodeAttrs {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut pairs = Vec::new();
        while !input.is_empty() {
            let name: Ident = input.parse()?;
            let _eq: syn::Token![=] = input.parse()?;
            // value 可以是字面量 (0, 5, "str")、标识符 (Custom, LoadFromPool)、
            // 方括号组 ([Reg, Const]) 或路径表达式 (nuzo_proc::xxx)
            let value = if input.peek(syn::LitStr) {
                let s: syn::LitStr = input.parse()?;
                s.value()
            } else if input.peek(syn::LitInt) {
                let i: syn::LitInt = input.parse()?;
                i.to_string()
            } else if input.peek(syn::token::Bracket) {
                // 处理 [Reg, Const] 等方括号组
                let content;
                syn::bracketed!(content in input);
                let tokens: proc_macro2::TokenStream = content.parse()?;
                let inner = tokens.to_string();
                // 去掉空格，保持紧凑
                let compact: String = inner.chars().filter(|c| !c.is_whitespace()).collect();
                format!("[{}]", compact)
            } else {
                // 尝试解析路径 (如 LoadFromPool, nuzo_proc::Custom) 或普通标识符
                // Path 涵盖了单段 Ident 和多段路径
                let p: syn::Path = input.parse()?;
                p.segments.iter().map(|s| s.ident.to_string()).collect::<Vec<_>>().join("::")
            };
            pairs.push(KeyValuePair { name: name.to_string(), value });
            // 可选逗号
            let _ = input.parse::<syn::Token![,]>();
        }
        Ok(OpcodeAttrs { pairs })
    }
}
