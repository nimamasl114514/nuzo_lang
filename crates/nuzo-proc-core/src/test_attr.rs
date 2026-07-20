//! `#[nuzo_test]` 属性宏核心展开逻辑
//!
//! 将声明式测试参数转换为完整的测试函数体生成。

use quote::quote;
use syn::ItemFn;

/// `#[nuzo_test]` 的解析结果（使用原生 Rust 类型，便于代码生成）。
pub struct NuzoTestInput {
    pub source: String,
    pub expect_output: Option<Vec<String>>,
    pub expect_exit_code: Option<i64>,
    pub expect_error_contains: Option<Vec<String>>,
}

/// 从 Meta 列表中提取 `#[nuzo_test(...)]` 参数。
///
/// 使用共享的 [`crate::parse_utils`] 消除重复解析代码。
pub fn parse_nuzo_test_attrs(meta_list: &[syn::Meta]) -> syn::Result<NuzoTestInput> {
    use crate::parse_utils::{extract_string_array, parse_int_lit, parse_string_lit};

    let mut source_val: Option<String> = None;
    let mut expect_output_vals: Option<Vec<String>> = None;
    let mut expect_exit_code_val: Option<i64> = None;
    let mut expect_error_contains_vals: Option<Vec<String>> = None;

    for meta in meta_list {
        match meta {
            syn::Meta::NameValue(nv) => {
                if nv.path.is_ident("source") {
                    source_val = Some(parse_string_lit(&nv.value)?);
                } else if nv.path.is_ident("expect_output") {
                    // extract_string_array 返回 Vec<LitStr>，需要转 Vec<String>
                    let lits = extract_string_array(&nv.value)?;
                    expect_output_vals = Some(lits.iter().map(|s| s.value()).collect());
                } else if nv.path.is_ident("expect_exit_code") {
                    expect_exit_code_val = Some(parse_int_lit(&nv.value, "exit_code", "")?);
                } else if nv.path.is_ident("expect_error_contains") {
                    let lits = extract_string_array(&nv.value)?;
                    expect_error_contains_vals = Some(lits.iter().map(|s| s.value()).collect());
                } else {
                    return Err(crate::diag::SpannedError::new_spanned(
                        nv,
                        format!("unknown parameter `{}`; expected: source, expect_output, expect_exit_code, expect_error_contains", quote!(#nv.path)),
                    ).into_inner());
                }
            }
            other => {
                return Err(crate::diag::SpannedError::new_spanned(
                    other,
                    "expected `key = value` format",
                )
                .into_inner());
            }
        }
    }

    let source = source_val.ok_or_else(|| {
        crate::diag::SpannedError::new(
            proc_macro2::Span::call_site(),
            "missing required parameter `source`; usage: #[nuzo_test(source = \"...\")]",
        )
        .into_inner()
    })?;

    Ok(NuzoTestInput {
        source,
        expect_output: expect_output_vals,
        expect_exit_code: expect_exit_code_val,
        expect_error_contains: expect_error_contains_vals,
    })
}

/// 核心展开：从解析后的输入生成测试函数体 TokenStream。
pub fn expand_nuzo_test_attr(
    item: &ItemFn,
    input: NuzoTestInput,
) -> syn::Result<proc_macro2::TokenStream> {
    let vis = &item.vis;
    let sig = &item.sig;
    let attrs = &item.attrs;
    let source_str = &input.source;
    let expect_output = input.expect_output;
    let expect_exit_code = input.expect_exit_code;
    let expect_error_contains = input.expect_error_contains;

    let mut assertions: Vec<proc_macro2::TokenStream> = Vec::new();

    // 退出码断言
    if let Some(exit_code) = expect_exit_code {
        assertions.push(quote! {
            match (&result, #exit_code) {
                (Ok(_), code) if code != 0 => {
                    std::panic!(
                        "nuzo_test FAILED [exit code]:\n  \
                         Expected exit code {} but execution succeeded\n  \
                         Source:\n{}",
                        code, #source_str
                    );
                }
                (Err(e), 0) => {
                    std::panic!(
                        "nuzo_test FAILED [exit code]:\n  \
                         Expected success (exit code 0) but got error: {}\n  \
                         Source:\n{}",
                        e, #source_str
                    );
                }
                _ => {}
            }
        });
    }

    // 输出内容断言
    if let Some(ref expected_lines) = expect_output {
        let line_assertions: Vec<proc_macro2::TokenStream> = expected_lines
            .iter()
            .map(|line| {
                let line_str = line.as_str();
                quote! {
                    {
                        let expected_line: &str = #line_str;
                        if !output_str.contains(expected_line) {
                            std::panic!(
                                "nuzo_test FAILED [output mismatch]:\n  \
                                 Expected output to contain \"{}\"\n  \
                                 Actual output:\n{}\n  \
                                 Source:\n{}",
                                expected_line, output_str, #source_str
                            );
                        }
                    }
                }
            })
            .collect();

        assertions.push(quote! {
            let output_str = output_lines.join("\n");
            #(#line_assertions)*
        });
    }

    // 错误信息断言
    if let Some(ref err_patterns) = expect_error_contains {
        let pattern_assertions: Vec<proc_macro2::TokenStream> = err_patterns
            .iter()
            .map(|pat| {
                let pat_str = pat.as_str();
                quote! {
                    {
                        let err_pattern: &str = #pat_str;
                        if !err_str.contains(err_pattern) {
                            std::panic!(
                                "nuzo_test FAILED [error pattern mismatch]:\n  \
                                 Expected error to contain \"{}\"\n  \
                                 Actual error: {}\n  \
                                 Source:\n{}",
                                err_pattern, err_str, #source_str
                            );
                        }
                    }
                }
            })
            .collect();

        assertions.push(quote! {
            match &result {
                Ok(_) => {
                    std::panic!(
                        "nuzo_test FAILED [expected error]:\n  \
                         Expected error containing specified patterns but execution succeeded\n  \
                         Source:\n{}",
                        #source_str
                    );
                }
                Err(e) => {
                    let err_str = format!("{}", e);
                    #(#pattern_assertions)*
                }
            }
        });
    }

    let body = if assertions.is_empty() {
        quote! {
            let (output_lines, result) =
                $crate::nuzo_testkit::nuzo_test_macro::execute_nuzo_source(#source_str);
            #[allow(unused_variables)]
            let _output_lines = &output_lines;
            #[allow(unused_variables)]
            let _result = &result;
        }
    } else {
        quote! {
            let (output_lines, result) =
                $crate::nuzo_testkit::nuzo_test_macro::execute_nuzo_source(#source_str);
            #(#assertions)*
        }
    };

    Ok(quote! {
        #(#attrs)*
        #vis #sig {
            #body
        }
    })
}
