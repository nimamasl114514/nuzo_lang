//! `test_bind::py_test!` 宏的核心展开逻辑。
//!
//! 使用 `rustpython-parser` 的完整 Python AST 将 Python 风格测试代码
//! 转换为 Rust `#[test]` 函数。支持 if/for/while/变量赋值/函数调用等复杂逻辑。

use proc_macro2::TokenStream;
use quote::quote;
use rustpython_parser::Parse;
use rustpython_parser::ast::{
    self, BoolOp, CmpOp, Constant, Expr, Keyword, Operator, Stmt, UnaryOp,
};
use syn::LitStr;

/// 将 Python 风格测试代码展开为 Rust `#[test]` 函数集合。
///
/// 支持的语法子集见模块级文档。输入必须是 `syn::LitStr`（过程宏中传入的字符串字面量）。
pub fn expand_py_test(source: &LitStr) -> syn::Result<TokenStream> {
    let src = source.value();
    let span = source.span();

    if src.trim().is_empty() {
        return Err(syn::Error::new(span, "py_test! 不接受空输入"));
    }

    let suite: ast::Suite = ast::Suite::parse(&src, "<py_test>")
        .map_err(|e| syn::Error::new(span, format!("Python 语法解析失败: {e}")))?;

    if suite.is_empty() {
        return Err(syn::Error::new(span, "py_test! 中未找到函数定义"));
    }

    let rust_fns = convert_suite(&suite, span)?;

    // 5. 组装最终输出（仅生成 #[test] 函数，不自动导入 bind 避免冲突）
    Ok(quote! {
        #[cfg(test)]
        #[allow(unused_imports, unused_variables, unused_mut)]
        #(#rust_fns)*
    })
}

/// 将顶层语句列表转换为 Rust TokenStream 列表。
/// 顶层只允许 FunctionDef，其他语句报错。
fn convert_suite(stmts: &[Stmt], span: proc_macro2::Span) -> syn::Result<Vec<TokenStream>> {
    let mut result = Vec::new();
    for stmt in stmts {
        match stmt {
            Stmt::FunctionDef(func_def) => {
                let ts = convert_function_def(func_def, span)?;
                result.push(ts);
            }
            _ => {
                return Err(syn::Error::new(
                    span,
                    format!("顶层只允许函数定义，不支持 {}", stmt_name(stmt)),
                ));
            }
        }
    }
    if result.is_empty() {
        return Err(syn::Error::new(span, "py_test! 中未找到函数定义"));
    }
    Ok(result)
}

/// 获取语句的可读名称，用于错误信息。
fn stmt_name(stmt: &Stmt) -> &'static str {
    match stmt {
        Stmt::FunctionDef(_) => "函数定义",
        Stmt::AsyncFunctionDef(_) => "异步函数定义",
        Stmt::ClassDef(_) => "类定义",
        Stmt::Return(_) => "return 语句",
        Stmt::Delete(_) => "delete 语句",
        Stmt::Assign(_) => "赋值语句",
        Stmt::TypeAlias(_) => "类型别名",
        Stmt::AugAssign(_) => "增强赋值语句",
        Stmt::AnnAssign(_) => "注解赋值语句",
        Stmt::For(_) => "for 循环",
        Stmt::AsyncFor(_) => "异步 for 循环",
        Stmt::While(_) => "while 循环",
        Stmt::If(_) => "if 语句",
        Stmt::With(_) => "with 语句",
        Stmt::AsyncWith(_) => "异步 with 语句",
        Stmt::Match(_) => "match 语句",
        Stmt::Raise(_) => "raise 语句",
        Stmt::Try(_) => "try 语句",
        Stmt::TryStar(_) => "try* 语句",
        Stmt::Assert(_) => "assert 语句",
        Stmt::Import(_) => "import 语句",
        Stmt::ImportFrom(_) => "from import 语句",
        Stmt::Global(_) => "global 语句",
        Stmt::Nonlocal(_) => "nonlocal 语句",
        Stmt::Expr(_) => "表达式语句",
        Stmt::Pass(_) => "pass 语句",
        Stmt::Break(_) => "break 语句",
        Stmt::Continue(_) => "continue 语句",
    }
}

/// 将 Python 函数定义转换为 `#[test] fn name() { ... }`。
fn convert_function_def(
    func_def: &ast::StmtFunctionDef,
    span: proc_macro2::Span,
) -> syn::Result<TokenStream> {
    let fn_name_str = func_def.name.as_str();
    if fn_name_str.is_empty() {
        return Err(syn::Error::new(span, "函数名为空"));
    }
    let fn_ident = syn::Ident::new(fn_name_str, span);

    // 检查函数参数——py_test! 函数不应有参数
    let args = &func_def.args;
    let has_args = !args.posonlyargs.is_empty()
        || !args.args.is_empty()
        || args.vararg.is_some()
        || !args.kwonlyargs.is_empty()
        || args.kwarg.is_some();
    if has_args {
        return Err(syn::Error::new(span, format!("py_test! 函数 `{fn_name_str}` 不应有参数")));
    }

    let body_stmts = convert_stmts(&func_def.body, span)?;

    Ok(quote! {
        #[test]
        fn #fn_ident() {
            #(#body_stmts)*
        }
    })
}

/// 将语句列表转换为 TokenStream 列表。
fn convert_stmts(stmts: &[Stmt], span: proc_macro2::Span) -> syn::Result<Vec<TokenStream>> {
    stmts.iter().map(|s| convert_stmt(s, span)).collect()
}

/// 将单个 Python 语句转换为 Rust TokenStream。
fn convert_stmt(stmt: &Stmt, span: proc_macro2::Span) -> syn::Result<TokenStream> {
    match stmt {
        Stmt::FunctionDef(_) => Err(syn::Error::new(span, "py_test! 不支持嵌套函数定义")),
        Stmt::AsyncFunctionDef(_) => Err(syn::Error::new(span, "py_test! 不支持异步函数定义")),
        Stmt::ClassDef(_) => Err(syn::Error::new(span, "py_test! 不支持 class 定义")),

        Stmt::Assign(assign) => {
            if assign.targets.len() != 1 {
                return Err(syn::Error::new(span, "py_test! 不支持多重赋值 (a = b = c)"));
            }
            let target = convert_expr(&assign.targets[0], span)?;
            let value = convert_expr(&assign.value, span)?;
            Ok(quote! { let mut #target = #value; })
        }

        Stmt::AugAssign(aug) => {
            let target = convert_expr(&aug.target, span)?;
            let op = convert_aug_operator(&aug.op);
            let value = convert_expr(&aug.value, span)?;
            Ok(quote! { #target #op #value; })
        }

        Stmt::If(if_stmt) => convert_if(if_stmt, span),

        Stmt::For(for_stmt) => convert_for(for_stmt, span),

        Stmt::While(while_stmt) => {
            let test = convert_expr(&while_stmt.test, span)?;
            let body = convert_stmts(&while_stmt.body, span)?;
            if !while_stmt.orelse.is_empty() {
                return Err(syn::Error::new(span, "py_test! 不支持 while...else"));
            }
            Ok(quote! { while #test { #(#body)* } })
        }

        Stmt::Assert(assert_stmt) => convert_assert(&assert_stmt.test, &assert_stmt.msg, span),

        Stmt::Expr(expr_stmt) => {
            let value = convert_expr(&expr_stmt.value, span)?;
            Ok(quote! { #value; })
        }

        Stmt::Pass(_) => Ok(quote! {}),

        Stmt::Break(_) => Ok(quote! { break; }),

        Stmt::Continue(_) => Ok(quote! { continue; }),

        Stmt::Return(ret) => match &ret.value {
            Some(val) => {
                let value = convert_expr(val, span)?;
                Ok(quote! { return #value; })
            }
            None => Ok(quote! { return; }),
        },

        Stmt::Delete(_) => Err(syn::Error::new(span, "py_test! 不支持 delete 语句")),
        Stmt::TypeAlias(_) => Err(syn::Error::new(span, "py_test! 不支持类型别名")),
        Stmt::AnnAssign(_) => Err(syn::Error::new(span, "py_test! 不支持注解赋值")),
        Stmt::AsyncFor(_) => Err(syn::Error::new(span, "py_test! 不支持异步 for 循环")),
        Stmt::With(_) => Err(syn::Error::new(span, "py_test! 不支持 with 语句")),
        Stmt::AsyncWith(_) => Err(syn::Error::new(span, "py_test! 不支持异步 with 语句")),
        Stmt::Match(_) => Err(syn::Error::new(span, "py_test! 不支持 match 语句")),
        Stmt::Raise(_) => Err(syn::Error::new(span, "py_test! 不支持 raise 语句")),
        Stmt::Try(_) => Err(syn::Error::new(span, "py_test! 不支持 try 语句")),
        Stmt::TryStar(_) => Err(syn::Error::new(span, "py_test! 不支持 try* 语句")),
        Stmt::Import(_) => Err(syn::Error::new(span, "py_test! 不支持 import 语句")),
        Stmt::ImportFrom(_) => Err(syn::Error::new(span, "py_test! 不支持 from import 语句")),
        Stmt::Global(_) => Err(syn::Error::new(span, "py_test! 不支持 global 语句")),
        Stmt::Nonlocal(_) => Err(syn::Error::new(span, "py_test! 不支持 nonlocal 语句")),
    }
}

/// 将 Python if/elif/else 转换为 Rust if/else if/else。
fn convert_if(if_stmt: &ast::StmtIf, span: proc_macro2::Span) -> syn::Result<TokenStream> {
    let test = convert_expr(&if_stmt.test, span)?;
    let body = convert_stmts(&if_stmt.body, span)?;

    if if_stmt.orelse.is_empty() {
        Ok(quote! { if #test { #(#body)* } })
    } else if if_stmt.orelse.len() == 1 && matches!(&if_stmt.orelse[0], Stmt::If(_)) {
        // elif: orelse 中包含一个 If 语句
        let else_if = convert_stmt(&if_stmt.orelse[0], span)?;
        Ok(quote! { if #test { #(#body)* } else #else_if })
    } else {
        let else_body = convert_stmts(&if_stmt.orelse, span)?;
        Ok(quote! { if #test { #(#body)* } else { #(#else_body)* } })
    }
}

/// 将 Python for 循环转换为 Rust for 循环。
/// 特殊处理 `range()` 调用：`for x in range(n)` → `for x in 0..n`。
fn convert_for(for_stmt: &ast::StmtFor, span: proc_macro2::Span) -> syn::Result<TokenStream> {
    let target = convert_expr(&for_stmt.target, span)?;
    let body = convert_stmts(&for_stmt.body, span)?;

    if !for_stmt.orelse.is_empty() {
        return Err(syn::Error::new(span, "py_test! 不支持 for...else"));
    }

    if let Some(range_args) = try_extract_range_args(&for_stmt.iter) {
        return convert_for_range(&target, &range_args, &body, span);
    }

    let iter = convert_expr(&for_stmt.iter, span)?;
    Ok(quote! { for #target in #iter { #(#body)* } })
}

/// 尝试从表达式中提取 range() 调用参数。
/// 返回 Some(args) 如果是 `range(...)` 调用。
fn try_extract_range_args(iter: &Expr) -> Option<Vec<Expr>> {
    match iter {
        Expr::Call(call) => {
            if let Expr::Name(name) = call.func.as_ref() {
                if name.id.as_str() == "range" && call.keywords.is_empty() {
                    return Some(call.args.clone());
                }
            }
            None
        }
        _ => None,
    }
}

/// 将 `for x in range(...)` 转换为 Rust range 表达式。
fn convert_for_range(
    target: &TokenStream,
    args: &[Expr],
    body: &[TokenStream],
    span: proc_macro2::Span,
) -> syn::Result<TokenStream> {
    match args.len() {
        1 => {
            let end = convert_expr(&args[0], span)?;
            Ok(quote! { for #target in 0..#end { #(#body)* } })
        }
        2 => {
            let start = convert_expr(&args[0], span)?;
            let end = convert_expr(&args[1], span)?;
            Ok(quote! { for #target in #start..#end { #(#body)* } })
        }
        3 => {
            let start = convert_expr(&args[0], span)?;
            let end = convert_expr(&args[1], span)?;
            let step = convert_expr(&args[2], span)?;
            Ok(quote! { for #target in (#start..#end).step_by(#step) { #(#body)* } })
        }
        _ => Err(syn::Error::new(span, format!("range() 需要 1~3 个参数，收到 {} 个", args.len()))),
    }
}

/// 智能选择 assert!/assert_eq!/assert_ne!。
fn convert_assert(
    test: &Expr,
    msg: &Option<Box<Expr>>,
    span: proc_macro2::Span,
) -> syn::Result<TokenStream> {
    if let Some((left, right)) = is_simple_eq_compare(test) {
        let l = convert_expr(left, span)?;
        let r = convert_expr(right, span)?;
        if let Some(msg_expr) = msg {
            let msg_ts = convert_expr(msg_expr, span)?;
            Ok(quote! { assert_eq!(#l, #r, #msg_ts); })
        } else {
            Ok(quote! { assert_eq!(#l, #r); })
        }
    } else if let Some((left, right)) = is_simple_ne_compare(test) {
        let l = convert_expr(left, span)?;
        let r = convert_expr(right, span)?;
        if let Some(msg_expr) = msg {
            let msg_ts = convert_expr(msg_expr, span)?;
            Ok(quote! { assert_ne!(#l, #r, #msg_ts); })
        } else {
            Ok(quote! { assert_ne!(#l, #r); })
        }
    } else {
        let expr = convert_expr(test, span)?;
        if let Some(msg_expr) = msg {
            let msg_ts = convert_expr(msg_expr, span)?;
            Ok(quote! { assert!(#expr, #msg_ts); })
        } else {
            Ok(quote! { assert!(#expr); })
        }
    }
}

/// 检测 `a == b` 模式（单个 Eq 比较，无链式）。
fn is_simple_eq_compare(expr: &Expr) -> Option<(&Expr, &Expr)> {
    match expr {
        Expr::Compare(cmp) => {
            if cmp.ops.len() == 1 && matches!(cmp.ops[0], CmpOp::Eq) && cmp.comparators.len() == 1 {
                Some((&cmp.left, &cmp.comparators[0]))
            } else {
                None
            }
        }
        _ => None,
    }
}

/// 检测 `a != b` 模式（单个 NotEq 比较，无链式）。
fn is_simple_ne_compare(expr: &Expr) -> Option<(&Expr, &Expr)> {
    match expr {
        Expr::Compare(cmp) => {
            if cmp.ops.len() == 1
                && matches!(cmp.ops[0], CmpOp::NotEq)
                && cmp.comparators.len() == 1
            {
                Some((&cmp.left, &cmp.comparators[0]))
            } else {
                None
            }
        }
        _ => None,
    }
}

/// 将 Python 表达式转换为 Rust TokenStream。
fn convert_expr(expr: &Expr, span: proc_macro2::Span) -> syn::Result<TokenStream> {
    match expr {
        Expr::BoolOp(bool_op) => {
            let op = convert_boolop(&bool_op.op);
            let values: Vec<TokenStream> =
                bool_op.values.iter().map(|v| convert_expr(v, span)).collect::<syn::Result<_>>()?;
            // 用运算符连接所有值: a && b && c
            let mut iter = values.into_iter();
            let first = iter.next().unwrap_or_else(|| quote! { true });
            let result = iter.fold(first, |acc, v| quote! { (#acc) #op (#v) });
            Ok(result)
        }

        Expr::BinOp(bin_op) => {
            let left = convert_expr(&bin_op.left, span)?;
            let op = convert_operator(&bin_op.op);
            let right = convert_expr(&bin_op.right, span)?;
            match &bin_op.op {
                Operator::FloorDiv => Ok(quote! { (#left) / (#right) }),
                Operator::Pow => {
                    Err(syn::Error::new(span, "py_test! 不支持 ** 运算符，请使用 .pow() 方法"))
                }
                Operator::MatMult => Err(syn::Error::new(span, "py_test! 不支持矩阵乘法 @ 运算符")),
                _ => Ok(quote! { (#left) #op (#right) }),
            }
        }

        Expr::UnaryOp(unary_op) => {
            let operand = convert_expr(&unary_op.operand, span)?;
            match &unary_op.op {
                UnaryOp::Not => Ok(quote! { (!(#operand)) }),
                UnaryOp::USub => Ok(quote! { (-(#operand)) }),
                UnaryOp::UAdd => Ok(quote! { (+(#operand)) }),
                UnaryOp::Invert => Ok(quote! { (!(#operand)) }),
            }
        }

        Expr::Compare(cmp) => {
            // 链式比较: a < b < c → a < b && b < c
            let left = convert_expr(&cmp.left, span)?;
            if cmp.ops.len() == 1 {
                let op = convert_cmpop(&cmp.ops[0]);
                let right = convert_expr(&cmp.comparators[0], span)?;
                Ok(quote! { (#left) #op (#right) })
            } else {
                let mut parts = Vec::new();
                let mut prev: &Expr = &cmp.left;
                for (i, op) in cmp.ops.iter().enumerate() {
                    let op_ts = convert_cmpop(op);
                    let curr = &cmp.comparators[i];
                    let prev_ts = convert_expr(prev, span)?;
                    let curr_ts = convert_expr(curr, span)?;
                    parts.push(quote! { (#prev_ts) #op_ts (#curr_ts) });
                    prev = curr;
                }
                Ok(quote! { #(#parts)&&* })
            }
        }

        Expr::Call(call) => convert_call(&call.func, &call.args, &call.keywords, span),

        Expr::Constant(const_expr) => convert_constant(&const_expr.value, span),

        Expr::Name(name) => {
            let id_str = name.id.as_str();
            // Python True/False/None 在 AST 中已经是 Constant，不会走到这里
            // 但保留防御性检查
            let ident = syn::Ident::new(id_str, span);
            Ok(quote! { #ident })
        }

        Expr::Attribute(attr) => {
            let value = convert_expr(&attr.value, span)?;
            let attr_str = attr.attr.as_str();

            match attr_str {
                "append" => {
                    // x.append(v) 在 convert_call 中处理
                    let attr_ident = syn::Ident::new(attr_str, span);
                    Ok(quote! { #value.#attr_ident })
                }
                _ => {
                    let attr_ident = syn::Ident::new(attr_str, span);
                    Ok(quote! { #value.#attr_ident })
                }
            }
        }

        Expr::Subscript(sub) => {
            let value = convert_expr(&sub.value, span)?;
            let slice = convert_expr(&sub.slice, span)?;
            Ok(quote! { #value[#slice] })
        }

        Expr::List(list) => {
            let elts: Vec<TokenStream> =
                list.elts.iter().map(|e| convert_expr(e, span)).collect::<syn::Result<_>>()?;
            Ok(quote! { vec![#(#elts),*] })
        }

        Expr::Tuple(tuple) => {
            let elts: Vec<TokenStream> =
                tuple.elts.iter().map(|e| convert_expr(e, span)).collect::<syn::Result<_>>()?;
            Ok(quote! { (#(#elts),*) })
        }

        Expr::IfExp(if_exp) => {
            let test = convert_expr(&if_exp.test, span)?;
            let body = convert_expr(&if_exp.body, span)?;
            let orelse = convert_expr(&if_exp.orelse, span)?;
            Ok(quote! { if #test { #body } else { #orelse } })
        }

        Expr::NamedExpr(_) => Err(syn::Error::new(span, "py_test! 不支持海象运算符 :=")),
        Expr::Lambda(_) => Err(syn::Error::new(span, "py_test! 不支持 lambda 表达式")),
        Expr::Dict(_) => Err(syn::Error::new(span, "py_test! 不支持字典字面量")),
        Expr::Set(_) => Err(syn::Error::new(span, "py_test! 不支持集合字面量")),
        Expr::ListComp(_) => Err(syn::Error::new(span, "py_test! 不支持列表推导式")),
        Expr::SetComp(_) => Err(syn::Error::new(span, "py_test! 不支持集合推导式")),
        Expr::DictComp(_) => Err(syn::Error::new(span, "py_test! 不支持字典推导式")),
        Expr::GeneratorExp(_) => Err(syn::Error::new(span, "py_test! 不支持生成器表达式")),
        Expr::Await(_) => Err(syn::Error::new(span, "py_test! 不支持 await 表达式")),
        Expr::Yield(_) => Err(syn::Error::new(span, "py_test! 不支持 yield 表达式")),
        Expr::YieldFrom(_) => Err(syn::Error::new(span, "py_test! 不支持 yield from 表达式")),
        Expr::FormattedValue(_) => Err(syn::Error::new(span, "py_test! 不支持 f-string 格式化值")),
        Expr::JoinedStr(_) => Err(syn::Error::new(span, "py_test! 不支持 f-string")),
        Expr::Starred(_) => Err(syn::Error::new(span, "py_test! 不支持星号表达式")),
        Expr::Slice(_) => Err(syn::Error::new(span, "py_test! 不支持切片表达式")),
    }
}

/// 将 Python 常量转换为 Rust TokenStream。
fn convert_constant(value: &Constant, span: proc_macro2::Span) -> syn::Result<TokenStream> {
    match value {
        Constant::None => Ok(quote! { None }),
        Constant::Bool(b) => {
            if *b {
                Ok(quote! { true })
            } else {
                Ok(quote! { false })
            }
        }
        Constant::Str(s) => {
            let lit = proc_macro2::Literal::string(s);
            Ok(TokenStream::from(proc_macro2::TokenTree::Literal(lit)))
        }
        Constant::Bytes(b) => {
            let bytes: Vec<u8> = b.clone();
            Ok(quote! { &[#(#bytes),*][..] })
        }
        Constant::Int(big_int) => {
            let s = big_int.to_string();
            let lit = proc_macro2::Literal::i64_suffixed(s.parse::<i64>().map_err(|_| {
                syn::Error::new(span, format!("整数 `{s}` 超出 i64 范围，py_test! 不支持大整数"))
            })?);
            Ok(TokenStream::from(proc_macro2::TokenTree::Literal(lit)))
        }
        Constant::Float(f) => {
            let lit = proc_macro2::Literal::f64_unsuffixed(*f);
            Ok(TokenStream::from(proc_macro2::TokenTree::Literal(lit)))
        }
        Constant::Complex { .. } => Err(syn::Error::new(span, "py_test! 不支持复数字面量")),
        Constant::Tuple(elts) => {
            let items: Vec<TokenStream> =
                elts.iter().map(|e| convert_constant(e, span)).collect::<syn::Result<_>>()?;
            Ok(quote! { (#(#items),*) })
        }
        Constant::Ellipsis => Err(syn::Error::new(span, "py_test! 不支持省略号 ...")),
    }
}

/// 将 Python 运算符转换为 Rust TokenStream。
fn convert_operator(op: &Operator) -> TokenStream {
    match op {
        Operator::Add => quote! { + },
        Operator::Sub => quote! { - },
        Operator::Mult => quote! { * },
        Operator::Div => quote! { / },
        Operator::Mod => quote! { % },
        Operator::LShift => quote! { << },
        Operator::RShift => quote! { >> },
        Operator::BitOr => quote! { | },
        Operator::BitXor => quote! { ^ },
        Operator::BitAnd => quote! { & },
        // FloorDiv 和 Pow 在 convert_expr 中特殊处理
        Operator::FloorDiv => quote! { / },
        Operator::Pow => quote! { .pow() },
        Operator::MatMult => quote! { @ },
    }
}

/// 将 Python 运算符转换为 Rust 复合赋值运算符（+=, -= 等）。
fn convert_aug_operator(op: &Operator) -> TokenStream {
    match op {
        Operator::Add => quote! { += },
        Operator::Sub => quote! { -= },
        Operator::Mult => quote! { *= },
        Operator::Div => quote! { /= },
        Operator::Mod => quote! { %= },
        Operator::LShift => quote! { <<= },
        Operator::RShift => quote! { >>= },
        Operator::BitOr => quote! { |= },
        Operator::BitXor => quote! { ^= },
        Operator::BitAnd => quote! { &= },
        Operator::FloorDiv => quote! { /= },
        Operator::Pow => quote! { .pow() }, // 不支持 **=
        Operator::MatMult => quote! { @= }, // 不支持 @=
    }
}

/// 将 Python 比较运算符转换为 Rust TokenStream。
fn convert_cmpop(op: &CmpOp) -> TokenStream {
    match op {
        CmpOp::Eq => quote! { == },
        CmpOp::NotEq => quote! { != },
        CmpOp::Lt => quote! { < },
        CmpOp::LtE => quote! { <= },
        CmpOp::Gt => quote! { > },
        CmpOp::GtE => quote! { >= },
        CmpOp::Is => quote! { == }, // Python is 近似为 ==
        CmpOp::IsNot => quote! { != },
        CmpOp::In => quote! { .contains() }, // 需要特殊处理，这里仅占位
        CmpOp::NotIn => quote! { !.contains() },
    }
}

/// 将 Python 布尔运算符转换为 Rust TokenStream。
fn convert_boolop(op: &BoolOp) -> TokenStream {
    match op {
        BoolOp::And => quote! { && },
        BoolOp::Or => quote! { || },
    }
}

/// 将 Python 函数调用转换为 Rust TokenStream。
/// 特殊处理内置函数：print/len/range/str/int/float/abs/min/max/type/isinstance。
fn convert_call(
    func: &Expr,
    args: &[Expr],
    keywords: &[Keyword],
    span: proc_macro2::Span,
) -> syn::Result<TokenStream> {
    // 不支持关键字参数（除 print 外）
    if !keywords.is_empty() {
        // 检查是否是 print 调用的 end= 关键字
        if let Expr::Name(name) = func {
            if name.id.as_str() == "print" {
                // 简化处理：忽略关键字参数
                return convert_print_call(args, span);
            }
        }
        return Err(syn::Error::new(span, "py_test! 不支持关键字参数"));
    }

    match func {
        Expr::Name(name) => {
            let fn_name = name.id.as_str();
            match fn_name {
                "print" => convert_print_call(args, span),
                "len" => convert_len_call(args, span),
                "range" => convert_range_call(args, span),
                "str" => convert_str_call(args, span),
                "int" => convert_int_call(args, span),
                "float" => convert_float_call(args, span),
                "abs" => convert_method_call(args, "abs", span),
                "min" => convert_min_max_call(args, "min", span),
                "max" => convert_min_max_call(args, "max", span),
                "type" => Err(syn::Error::new(span, "py_test! 不支持 type() 函数")),
                "isinstance" => Err(syn::Error::new(span, "py_test! 不支持 isinstance() 函数")),
                _ => {
                    let fn_ident = syn::Ident::new(fn_name, span);
                    let converted_args: Vec<TokenStream> =
                        args.iter().map(|a| convert_expr(a, span)).collect::<syn::Result<_>>()?;
                    Ok(quote! { #fn_ident(#(#converted_args),*) })
                }
            }
        }
        Expr::Attribute(attr) => {
            let value = convert_expr(&attr.value, span)?;
            let method = attr.attr.as_str();
            let converted_args: Vec<TokenStream> =
                args.iter().map(|a| convert_expr(a, span)).collect::<syn::Result<_>>()?;

            match method {
                "append" => Ok(quote! { #value.push(#(#converted_args),*) }),
                _ => {
                    let method_ident = syn::Ident::new(method, span);
                    Ok(quote! { #value.#method_ident(#(#converted_args),*) })
                }
            }
        }
        _ => {
            let func_ts = convert_expr(func, span)?;
            let converted_args: Vec<TokenStream> =
                args.iter().map(|a| convert_expr(a, span)).collect::<syn::Result<_>>()?;
            Ok(quote! { #func_ts(#(#converted_args),*) })
        }
    }
}

/// `print(...)` → `println!(...)`
/// Python 的 print 多参数用空格分隔，Rust 的 println! 用格式化字符串。
fn convert_print_call(args: &[Expr], span: proc_macro2::Span) -> syn::Result<TokenStream> {
    if args.is_empty() {
        return Ok(quote! { println!(); });
    }

    if args.len() == 1 {
        if let Expr::Constant(const_expr) = &args[0] {
            if let Constant::Str(s) = &const_expr.value {
                if s.contains('{') && s.contains('}') {
                    let lit = proc_macro2::Literal::string(s);
                    return Ok(quote! { println!(#lit); });
                }
                let lit = proc_macro2::Literal::string(s);
                return Ok(quote! { println!(#lit); });
            }
        }
    }

    // 多参数或非字符串参数：转为 println!("{}", ...)
    let placeholders: Vec<&str> = args.iter().map(|_| "{}").collect();
    let fmt_str = placeholders.join(" ");
    let fmt_lit = proc_macro2::Literal::string(&fmt_str);
    let converted_args: Vec<TokenStream> =
        args.iter().map(|a| convert_expr(a, span)).collect::<syn::Result<_>>()?;
    Ok(quote! { println!(#fmt_lit, #(#converted_args),*); })
}

/// `len(x)` → `x.len()`
fn convert_len_call(args: &[Expr], span: proc_macro2::Span) -> syn::Result<TokenStream> {
    if args.len() != 1 {
        return Err(syn::Error::new(span, format!("len() 需要 1 个参数，收到 {} 个", args.len())));
    }
    let arg = convert_expr(&args[0], span)?;
    Ok(quote! { #arg.len() })
}

/// `range(...)` → Rust range 表达式（仅在表达式上下文，非 for 循环）
fn convert_range_call(args: &[Expr], span: proc_macro2::Span) -> syn::Result<TokenStream> {
    match args.len() {
        1 => {
            let end = convert_expr(&args[0], span)?;
            Ok(quote! { (0..#end) })
        }
        2 => {
            let start = convert_expr(&args[0], span)?;
            let end = convert_expr(&args[1], span)?;
            Ok(quote! { (#start..#end) })
        }
        3 => {
            let start = convert_expr(&args[0], span)?;
            let end = convert_expr(&args[1], span)?;
            let step = convert_expr(&args[2], span)?;
            Ok(quote! { (#start..#end).step_by(#step) })
        }
        _ => Err(syn::Error::new(span, format!("range() 需要 1~3 个参数，收到 {} 个", args.len()))),
    }
}

/// `str(x)` → `x.to_string()`
fn convert_str_call(args: &[Expr], span: proc_macro2::Span) -> syn::Result<TokenStream> {
    if args.len() != 1 {
        return Err(syn::Error::new(span, format!("str() 需要 1 个参数，收到 {} 个", args.len())));
    }
    let arg = convert_expr(&args[0], span)?;
    Ok(quote! { #arg.to_string() })
}

/// `int(x)` → `x as i64`
fn convert_int_call(args: &[Expr], span: proc_macro2::Span) -> syn::Result<TokenStream> {
    if args.len() != 1 {
        return Err(syn::Error::new(span, format!("int() 需要 1 个参数，收到 {} 个", args.len())));
    }
    let arg = convert_expr(&args[0], span)?;
    Ok(quote! { (#arg as i64) })
}

/// `float(x)` → `x as f64`
fn convert_float_call(args: &[Expr], span: proc_macro2::Span) -> syn::Result<TokenStream> {
    if args.len() != 1 {
        return Err(syn::Error::new(
            span,
            format!("float() 需要 1 个参数，收到 {} 个", args.len()),
        ));
    }
    let arg = convert_expr(&args[0], span)?;
    Ok(quote! { (#arg as f64) })
}

/// `abs(x)` → `x.abs()`, `min(a, b)` → `a.min(b)`, `max(a, b)` → `a.max(b)`
fn convert_method_call(
    args: &[Expr],
    method: &str,
    span: proc_macro2::Span,
) -> syn::Result<TokenStream> {
    if args.len() != 1 {
        return Err(syn::Error::new(
            span,
            format!("{method}() 需要 1 个参数，收到 {} 个", args.len()),
        ));
    }
    let arg = convert_expr(&args[0], span)?;
    let method_ident = syn::Ident::new(method, span);
    Ok(quote! { #arg.#method_ident() })
}

/// `min(a, b)` → `a.min(b)`, `max(a, b)` → `a.max(b)`
fn convert_min_max_call(
    args: &[Expr],
    method: &str,
    span: proc_macro2::Span,
) -> syn::Result<TokenStream> {
    if args.len() != 2 {
        return Err(syn::Error::new(
            span,
            format!("{method}() 需要 2 个参数，收到 {} 个", args.len()),
        ));
    }
    let a = convert_expr(&args[0], span)?;
    let b = convert_expr(&args[1], span)?;
    let method_ident = syn::Ident::new(method, span);
    Ok(quote! { #a.#method_ident(#b) })
}

#[cfg(test)]
mod tests {
    use super::*;
    use syn::parse_quote;

    /// 辅助：将 Python 源码转为展开后的 Rust 字符串（标准化空白）
    fn expand_str(py_src: &str) -> String {
        let lit: LitStr = parse_quote! { #py_src };
        let ts = expand_py_test(&lit).expect("展开失败");
        normalize_spacing(&ts.to_string())
    }

    /// proc_macro2 的 Display 在 token 间插入空格（如 `fn test_add ()`）。
    /// 此函数移除所有空白，使断言不受 Display 格式影响。
    fn normalize_spacing(s: &str) -> String {
        s.chars().filter(|c| !c.is_whitespace()).collect()
    }

    /// 断言展开结果中包含给定子串（忽略标准化空白）
    macro_rules! assert_contains {
        ($result:expr, $expected:expr) => {
            let expected_norm = normalize_spacing($expected);
            assert!(
                $result.contains(&expected_norm),
                "expected output to contain: {:?}\nactual output: {:?}",
                $expected,
                $result
            );
        };
    }

    #[test]
    fn basic_add() {
        let result = expand_str("def test_add():\n    assert 1 + 1 == 2\n");
        assert_contains!(result, "fn test_add()");
        assert_contains!(result, "assert_eq!((1i64) + (1i64), 2i64)");
    }

    #[test]
    fn assert_ne() {
        let result = expand_str("def test_ne():\n    assert 5 - 3 != 10\n");
        assert_contains!(result, "assert_ne!((5i64) - (3i64), 10i64)");
    }

    #[test]
    fn assert_bool() {
        let result = expand_str("def test_bool():\n    assert True\n");
        assert_contains!(result, "assert!(true)");
    }

    #[test]
    fn keywords() {
        let result = expand_str("def test_kw():\n    assert True and False or not None\n");
        assert_contains!(result, "true");
        assert_contains!(result, "&&");
        assert_contains!(result, "false");
        assert_contains!(result, "||");
        assert_contains!(result, "!");
        assert_contains!(result, "None");
    }

    #[test]
    fn comment() {
        // 注释行不导致编译错误
        let result =
            expand_str("def test_comment():\n    # this is a comment\n    assert 1 == 1\n");
        assert_contains!(result, "assert_eq!");
    }

    #[test]
    fn empty_input_error() {
        let lit: LitStr = parse_quote! { "" };
        let err = expand_py_test(&lit).unwrap_err();
        assert!(err.to_string().contains("空输入"));
    }

    #[test]
    fn unsupported_class() {
        let lit: LitStr = parse_quote! { "class Foo:\n    pass" };
        let err = expand_py_test(&lit).unwrap_err();
        assert!(err.to_string().contains("类定义") || err.to_string().contains("class"));
    }

    #[test]
    fn multiple_functions() {
        let result =
            expand_str("def test_a():\n    assert 1 == 1\n\ndef test_b():\n    assert 2 == 2\n");
        assert_contains!(result, "fn test_a()");
        assert_contains!(result, "fn test_b()");
    }

    #[test]
    fn word_boundary_not_in_identifier() {
        let result = expand_str("def test_note():\n    assert note == 1\n");
        assert_contains!(result, "note");
        let normalized = result;
        assert!(!normalized.contains("! e"), "should not contain '! e'");
    }

    #[test]
    fn logical_operators() {
        // assert 中的 == 在 BoolOp 内部，整体为 assert!
        let result = expand_str("def test_logic():\n    assert x > 0 and y < 10 or z == 5\n");
        assert_contains!(result, "&&");
        assert_contains!(result, "||");
        assert_contains!(result, "assert!");
    }

    #[test]
    fn none_passthrough() {
        let result = expand_str("def test_none():\n    assert result == None\n");
        assert_contains!(result, "assert_eq!(result, None)");
    }

    #[test]
    fn complex_expression() {
        let result = expand_str("def test_complex():\n    assert (1 + 2) * 3 != 10\n");
        assert_contains!(result, "assert_ne!");
        assert_contains!(result, "10");
    }

    #[test]
    fn if_statement() {
        let result = expand_str("def test_if():\n    if True:\n        assert 1 == 1\n");
        assert_contains!(result, "if true {");
        assert_contains!(result, "assert_eq!(1i64, 1i64)");
    }

    #[test]
    fn if_else() {
        let result = expand_str(
            "def test_if_else():\n    if True:\n        x = 1\n    else:\n        x = 2\n",
        );
        assert_contains!(result, "if true {");
        assert_contains!(result, "else {");
    }

    #[test]
    fn if_elif_else() {
        let result = expand_str(
            "def test_elif():\n    if x > 0:\n        y = 1\n    elif x < 0:\n        y = -1\n    else:\n        y = 0\n",
        );
        assert_contains!(result, "if");
        assert_contains!(result, "elseif");
        assert_contains!(result, "else");
    }

    #[test]
    fn for_range() {
        let result =
            expand_str("def test_for():\n    for i in range(10):\n        assert i >= 0\n");
        assert_contains!(result, "for i in 0..10i64");
    }

    #[test]
    fn for_range_two_args() {
        let result =
            expand_str("def test_for2():\n    for i in range(1, 10):\n        assert i >= 1\n");
        assert_contains!(result, "for i in 1i64..10i64");
    }

    #[test]
    fn for_range_step() {
        let result =
            expand_str("def test_for3():\n    for i in range(0, 10, 2):\n        assert i >= 0\n");
        assert_contains!(result, "(0i64..10i64).step_by(2i64)");
    }

    #[test]
    fn for_iter() {
        let result =
            expand_str("def test_for_iter():\n    for x in items:\n        assert x > 0\n");
        assert_contains!(result, "for x in items");
    }

    #[test]
    fn while_loop() {
        let result = expand_str("def test_while():\n    while x > 0:\n        x -= 1\n");
        assert_contains!(result, "while");
        assert_contains!(result, "x-=1i64;");
    }

    #[test]
    fn variable_assignment() {
        let result = expand_str("def test_assign():\n    x = 5\n    assert x == 5\n");
        assert_contains!(result, "let mut x = 5i64;");
        assert_contains!(result, "assert_eq!(x, 5i64)");
    }

    #[test]
    fn aug_assignment() {
        let result = expand_str("def test_aug():\n    x = 0\n    x += 1\n    assert x == 1\n");
        assert_contains!(result, "x += 1i64;");
    }

    #[test]
    fn print_call() {
        let result = expand_str("def test_print():\n    print(\"hello\")\n");
        assert_contains!(result, "println!");
    }

    #[test]
    fn len_call() {
        let result = expand_str("def test_len():\n    assert len(items) == 3\n");
        assert_contains!(result, "items.len()");
    }

    #[test]
    fn list_literal() {
        let result = expand_str("def test_list():\n    x = [1, 2, 3]\n    assert len(x) == 3\n");
        assert_contains!(result, "vec![1i64, 2i64, 3i64]");
    }

    #[test]
    fn method_append() {
        let result = expand_str("def test_append():\n    x = [1]\n    x.append(2)\n");
        assert_contains!(result, "x.push(2i64)");
    }

    #[test]
    fn break_continue() {
        let result = expand_str(
            "def test_bc():\n    for i in range(10):\n        if i == 5:\n            break\n        continue\n",
        );
        assert_contains!(result, "break;");
        assert_contains!(result, "continue;");
    }

    #[test]
    fn return_value() {
        let result = expand_str("def test_ret():\n    return 42\n");
        assert_contains!(result, "return 42i64;");
    }

    #[test]
    fn arithmetic_ops() {
        let result = expand_str("def test_ops():\n    assert 10 % 3 == 1\n");
        assert_contains!(result, "assert_eq!");
        assert_contains!(result, "%");
    }

    #[test]
    fn comparison_ops() {
        let result = expand_str("def test_cmp():\n    assert 5 > 3\n");
        assert_contains!(result, "assert!");
        assert_contains!(result, ">");
    }

    #[test]
    fn not_operator() {
        let result = expand_str("def test_not():\n    assert not False\n");
        assert_contains!(result, "assert!");
        assert_contains!(result, "!");
        assert_contains!(result, "false");
    }

    #[test]
    fn ternary_expr() {
        let result = expand_str("def test_ternary():\n    x = 1 if True else 0\n");
        assert_contains!(result, "if true { 1i64 } else { 0i64 }");
    }

    #[test]
    fn subscript() {
        let result = expand_str("def test_sub():\n    assert items[0] == 1\n");
        assert_contains!(result, "assert_eq!(items[0i64], 1i64)");
    }

    #[test]
    fn attribute_access() {
        let result = expand_str("def test_attr():\n    assert obj.value == 42\n");
        assert_contains!(result, "assert_eq!(obj.value, 42i64)");
    }

    #[test]
    fn tuple_literal() {
        let result = expand_str("def test_tuple():\n    x = (1, 2)\n    assert x == (1, 2)\n");
        assert_contains!(result, "let mut x = (1i64, 2i64);");
    }

    #[test]
    fn assert_with_message() {
        let result = expand_str("def test_msg():\n    assert 1 == 1, \"should equal\"\n");
        assert_contains!(result, "assert_eq!(1i64, 1i64, \"should equal\")");
    }

    #[test]
    fn str_call() {
        let result = expand_str("def test_str():\n    s = str(42)\n");
        assert_contains!(result, "to_string()");
    }

    #[test]
    fn abs_call() {
        let result = expand_str("def test_abs():\n    assert abs(-5) == 5\n");
        assert_contains!(result, ".abs()");
        assert_contains!(result, "assert_eq!");
    }

    #[test]
    fn min_max_call() {
        let result = expand_str(
            "def test_minmax():\n    assert min(1, 2) == 1\n    assert max(1, 2) == 2\n",
        );
        assert_contains!(result, ".min(");
        assert_contains!(result, ".max(");
    }

    #[test]
    fn nested_if_for() {
        let result = expand_str(
            "def test_nested():\n    for i in range(10):\n        if i > 5:\n            assert i > 5\n",
        );
        assert_contains!(result, "for i in 0..10i64");
        assert_contains!(result, "if");
        assert_contains!(result, ">");
    }

    #[test]
    fn no_functions_error() {
        let lit: LitStr = parse_quote! { "x = 5" };
        let err = expand_py_test(&lit).unwrap_err();
        assert!(err.to_string().contains("函数定义") || err.to_string().contains("顶层"));
    }

    #[test]
    fn pow_unsupported() {
        let lit: LitStr = parse_quote! { "def test_pow():\n    x = 2 ** 10\n" };
        let err = expand_py_test(&lit).unwrap_err();
        assert!(err.to_string().contains("**"));
    }

    #[test]
    fn import_unsupported() {
        let lit: LitStr = parse_quote! { "import os\ndef test_x():\n    pass\n" };
        let err = expand_py_test(&lit).unwrap_err();
        assert!(err.to_string().contains("import"));
    }
}
