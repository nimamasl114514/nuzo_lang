//! # nuzo_proc — Nuzo 过程宏入口
//!
//! **层级**: L1（过程宏入口层）—— 作为 `proc-macro` crate 暴露派生/属性/函数式宏入口，所有展开逻辑委托给 [`nuzo_proc_core`]。
//!
//! **主要入口**: [`derive_match_sync`], [`derive_trace`], [`derive_from_meta`], [`nuzo_test`], [`crate_meta`], [`define_opcodes!`], [`define_dispatch_auto!`], [`define_builtins!`]
//!
//! 本 crate 是 proc-macro 类型 crate，仅包含 `#[proc_macro_derive]` / `#[proc_macro_attribute]` 函数。
//! 所有核心展开逻辑委托给 [`nuzo_proc_core`]，本 crate 只做参数解析和转发。
//!
//! ## 架构约束
//! - **禁止在此实现宏逻辑** — 所有逻辑在 nuzo_proc_core 中
//! - **零运行时依赖** — 仅编译期使用
//! - **向前兼容** — 新增属性必须提供默认值或 deprecation 路径

use proc_macro::TokenStream;
use syn::{parse::Parser, punctuated::Punctuated, token::Comma};

/// 为枚举自动生成 Visitor 模式的 trait + dispatch 方法。
///
/// 详细文档见 [`nuzo_proc_core::match_sync`] 模块。
#[proc_macro_derive(MatchSync, attributes(match_sync))]
pub fn derive_match_sync(input: TokenStream) -> TokenStream {
    let input = syn::parse_macro_input!(input as syn::DeriveInput);
    nuzo_proc_core::match_sync::expand_match_sync(&input)
        .unwrap_or_else(|e| e.to_compile_error())
        .into()
}

/// 为 AST 节点枚举自动生成 Visitor 模式的默认子节点遍历方法。
///
/// 在 `Expr` 枚举上派生此宏，会自动生成 `visit_children_derived` 内建方法，
/// 行为等价于 `nuzo_frontend::ast::default_visit_expr`，但无需手写 40+ 行 match 分支。
///
/// ## 生成的代码
///
/// ```ignore
/// #[derive(ExprVisitor)]
/// pub enum Expr { ... }
///
/// // 生成：
/// impl Expr {
///     pub fn visit_children_derived<V: ExprVisitor + ?Sized>(&self, visitor: &mut V) {
///         match self { /* 自动生成的递归遍历 */ }
///     }
/// }
/// ```
///
/// ## MVP 支持范围
///
/// - 字面量变体（`Number`/`String`/`Bool`/`Nil`）→ `visit_literal`
/// - `Ident` 变体 → `visit_ident`
/// - `Fn`/`Closure` 变体 → `visit_fn`
/// - `Dict` 变体 → 遍历 `Vec<(String, Expr)>`
/// - `Match` 变体 → 遍历 scrutinee + arms 的 pattern/body
/// - 其余变体：按字段类型自动递归
///   - `Box<Expr>`、`Vec<Expr>`、`Option<Box<Expr>>`、`Block`、`Option<Block>`
///
/// 详细文档见 [`nuzo_proc_core::expr_visitor_derive`] 模块。
#[proc_macro_derive(ExprVisitor)]
pub fn derive_expr_visitor(input: TokenStream) -> TokenStream {
    let input = syn::parse_macro_input!(input as syn::DeriveInput);
    nuzo_proc_core::expr_visitor_derive::expand_expr_visitor(&input)
        .unwrap_or_else(|e| e.to_compile_error())
        .into()
}

/// 为结构体自动生成 `FromMeta` 实现（声明式属性解析）。
///
/// ## 支持的字段类型
///
/// | Rust 类型 | 属性语法 | 默认值 |
/// |-----------|---------|--------|
/// | `String` | `name = "value"` | 必填 |
/// | `bool` | `flag` 或 `flag = true/false` | `false` |
/// | `usize` | `count = 42` | 必填 |
/// | `i64` | `offset = -10` | 必填 |
/// | `Ident` | `kind = CustomName` | 必填 |
/// | `Path` | `ty = some::path` | 必填 |
/// | `Option<T>` | `opt = ...` | `None` |
/// | `Vec<T>` | `items = [a, b]` | `[]` |
/// | `f32` / `f64` | `ratio = 0.5` | 必填 |
/// | `char` | `sep = ','` | 必填 |
///
/// ## 字段属性
///
/// - `#[meta(default)]` — 标记可选字段（有默认值）
/// - `#[meta(rename = "attr_name")]` — 自定义属性名
///
/// ## 向前兼容
///
/// - 新增字段必须标注 `#[meta(default)]` 或提供编译期默认值
/// - 废弃字段通过 `#[deprecated]` + 默认值保留至少一个小版本
#[proc_macro_derive(FromMeta, attributes(meta))]
pub fn derive_from_meta(input: TokenStream) -> TokenStream {
    let input = syn::parse_macro_input!(input as syn::DeriveInput);
    nuzo_proc_core::attr::expand_from_meta_derive(&input)
        .unwrap_or_else(|e| e.to_compile_error())
        .into()
}

/// 为 GC HeapObject 枚举自动生成 `trace()` 方法实现。
///
/// 消除手写 match 分支的重复代码，确保新增变体不会遗漏 trace 调用。
///
/// ## 枚举级别属性：`#[trace(...)]`
///
/// | 属性 | 说明 | 默认值 |
/// |------|------|--------|
/// | `visitor` | visitor 参数类型路径 | `"Gc"` |
/// | `method_name` | 生成的方法名 | `"trace"` |
/// | `self_param` | self 参数形式 | `"&self"` |
/// | `visibility` | 方法可见性 | `"pub"` |
///
/// ## 变体级别属性：`#[trace(skip)]`
///
/// 标注该变体无需追踪（如不含堆引用的原始类型变体）。
///
/// ## 字段级别属性
///
/// - `#[trace(skip)]` — 跳过此字段的追踪
/// - `#[trace(field = "expr")]` — 自定义追踪表达式
///
/// ## 示例
///
/// ```ignore
/// #[derive(Trace)]
/// #[trace(visitor = "Gc")]
/// enum HeapObject {
///     String(Arc<str>),
///     Array(Vec<Value>),
///     Closure { captured: Vec<Value>, code_idx: u32 },
///     #[trace(skip)]
///     Int(i64),
/// }
/// ```
#[proc_macro_derive(Trace, attributes(trace))]
pub fn derive_trace(input: TokenStream) -> TokenStream {
    let input = syn::parse_macro_input!(input as syn::DeriveInput);
    nuzo_proc_core::trace_derive::expand_trace(&input)
        .unwrap_or_else(|e| e.to_compile_error())
        .into()
}

/// 为指令枚举自动生成 SSOT 宏和 dispatch 列表。
///
/// 消除 `with_every_instruction!` 和 `define_dispatch_auto!` 列表的手写重复，
/// 实现「改动一处 Instruction 枚举，自动同步全链路」。
///
/// ## 支持的属性
///
/// ### 枚举级 `#[opcode_meta(extra_dispatch = [...])]`
///
/// 声明不在枚举中但需要 dispatch handler 的 `Opcode` 变体
/// （如 VM 内部 patch 出来的 `GetGlobalCached`）。
///
/// ### 变体级 `#[opcode_meta(skip_ssot)]` / `#[opcode_meta(skip_dispatch)]`
///
/// - `skip_ssot` — 不纳入 `with_every_instruction!` SSOT
/// - `skip_dispatch` — 不纳入 `with_every_dispatch_opcode!` 列表
///
/// ## 生成内容
///
/// 1. `with_every_instruction!` — SSOT 宏（替代手写）
/// 2. `with_every_dispatch_opcode!` — dispatch 列表宏
/// 3. `INSTRUCTION_COUNT` — 指令总数常量
/// 4. 编译期断言 — 防止手动修改常量
///
/// ## 示例
///
/// ```ignore
/// #[derive(Debug, Clone, OpcodeSync)]
/// #[opcode_meta(extra_dispatch = [GetGlobalCached, SpillLoad, SpillStore])]
/// pub enum Instruction {
///     LoadK { dest: Reg, const_idx: ConstIdx },
///     #[opcode_meta(skip_ssot)]
///     Halt,
/// }
/// ```
#[proc_macro_derive(OpcodeSync, attributes(opcode_meta))]
pub fn derive_opcode_sync(input: TokenStream) -> TokenStream {
    let input = syn::parse_macro_input!(input as syn::DeriveInput);
    nuzo_proc_core::opcode_sync_derive::expand_opcode_sync(&input)
        .unwrap_or_else(|e| e.to_compile_error())
        .into()
}

/// 声明式测试属性宏。用于集成测试中定义 Nuzo 源码测试用例。
///
/// ## 参数
///
/// | 参数 | 类型 | 是否必填 | 说明 |
/// |------|------|---------|------|
/// | `source` | `&str` | **必填** | Nuzo 源代码字符串 |
/// | `expect_output` | `[&str; N]` | 可选 | 期望输出行列表 |
/// | `expect_exit_code` | 整数 | 可选 | 期望退出码 |
/// | `expect_error_contains` | `[&str; N]` | 可选 | 期望错误信息包含的模式列表 |
///
/// ## 示例
///
/// ```ignore
/// #[nuzo_test(
///     source = "print(1 + 2);",
///     expect_output = ["3"],
///     expect_exit_code = 0,
/// )]
/// fn test_addition() {}
/// ```
#[proc_macro_attribute]
pub fn nuzo_test(attr: TokenStream, item: TokenStream) -> TokenStream {
    let meta_list = if attr.is_empty() {
        vec![]
    } else {
        let parser = Punctuated::<syn::Meta, Comma>::parse_terminated;
        match parser.parse(attr.clone()) {
            Ok(punct) => punct.into_iter().collect(),
            Err(e) => return e.to_compile_error().into(),
        }
    };

    let input = nuzo_proc_core::test_attr::parse_nuzo_test_attrs(&meta_list);
    let parsed = match input {
        Ok(p) => p,
        Err(e) => return e.to_compile_error().into(),
    };

    let item_fn = syn::parse_macro_input!(item as syn::ItemFn);
    nuzo_proc_core::test_attr::expand_nuzo_test_attr(&item_fn, parsed)
        .unwrap_or_else(|e| e.to_compile_error())
        .into()
}

/// Crate 元数据属性宏（内层属性，附在 lib.rs 顶部）。
///
/// 用于在 crate 根部声明层级、描述、入口类型等元信息，
/// 编译期生成 `NUZO_CRATE_META` 常量，供 runtime 反射 / 文档生成使用。
///
/// ## 参数
///
/// | 参数 | 类型 | 必填 | 说明 |
/// |------|------|------|------|
/// | `layer` | `usize` | **必填** | 架构层级（0-4） |
/// | `description` | `str` | 可选 | crate 用途描述 |
/// | `entry_type` | `str` | 可选 | 入口类型（如 `Compiler` / `VM` / `Lib`） |
///
/// ## 示例
///
/// ```ignore
/// #![crate_meta(layer = 4, description = "编译器核心", entry_type = "Compiler")]
/// ```
///
/// ## 展开
///
/// 展开为 `pub const NUZO_CRATE_META: ...` 常量定义 + 原 item 透传。
/// 元数据生成逻辑见 [`nuzo_proc_core::crate_meta`]。
#[proc_macro_attribute]
pub fn crate_meta(
    attr: proc_macro::TokenStream,
    item: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
    let meta_tokens = nuzo_proc_core::crate_meta::expand_crate_meta(attr.into())
        .unwrap_or_else(|e| e.to_compile_error());
    let item: proc_macro2::TokenStream = item.into();
    quote::quote! {
        #meta_tokens
        #item
    }
    .into()
}

/// 声明式操作码定义宏。从属性化语法定义生成完整的 Opcode 枚举及方法实现。
///
/// ## 语法
///
/// ```ignore
/// define_opcodes! {
///     /// 文档注释会保留到生成的枚举变体上
///     #[opcode(code = 0x00, size = 1, operands = [], disasm = "halt", dispatch = Simple, desc = "...", summary = "...")]
///     Halt,
///
///     #[opcode(code = 0x01, size = 5, operands = [Reg, Const], disasm = "loadk {r}, {k}", dispatch = LoadK, desc = "...")]
///     LoadK,
/// }
/// ```
///
/// ## 属性说明 (`#[opcode(...)]`)
///
/// | 字段 | 类型 | 必填 | 说明 |
/// |------|------|------|------|
/// | `code` | `u8` | **必填** | 操作码数值（0-255） |
/// | `size` | `usize` | **必填** | 指令总字节大小 |
/// | `operands` | `[Ident]` | 可选 | 操作数类型列表（如 `[Reg, Const]`） |
/// | `disasm` | `str \| custom` | 可选 | 反汇编模板或 `custom` |
/// | `dispatch` | `Path` | 可选 | 分发策略标识符 |
/// | `desc` | `str` | 可选 | 完整描述文本 |
/// | `summary` | `str` | 可选 | 简短摘要文本 |
///
/// ## 向前兼容保证
///
/// - 新增操作码**只能追加**到末尾（不改变已有 code 编号）
/// - 新增属性字段必须有默认值
/// - 废弃操作码标记 `#[deprecated]` + 保留空桩实现
#[proc_macro]
pub fn define_opcodes(input: TokenStream) -> TokenStream {
    nuzo_codegen::opcode_gen::expand_define_opcodes(input.into())
        .unwrap_or_else(|e| e.to_compile_error())
        .into()
}

/// 自动生成 opcode handler 分发表和计数常量。
///
/// ## 语法
///
/// ```ignore
/// define_dispatch_auto! {
///     LoadK => _op_loadk,   // 显式指定 handler 名称
///     Add,                   // 自动推导为 _op_add
///     Sub,
/// }
/// ```
///
/// 生成：
/// ```ignore
/// pub const INSTRUCTION_COUNT: usize = 3;
/// pub fn get_handler(opcode: Opcode) -> Option<OpHandler> {
///     match opcode {
///         Opcode::LoadK => Some(_op_loadk),
///         Opcode::Add => Some(_op_add),
///         Opcode::Sub => Some(_op_sub),
///         _ => None,
///     }
/// }
/// ```
#[proc_macro]
pub fn define_dispatch_auto(input: TokenStream) -> TokenStream {
    nuzo_codegen::dispatch_gen::expand_define_dispatch_auto(input.into())
        .unwrap_or_else(|e| e.to_compile_error())
        .into()
}

/// 声明式 builtin 函数注册表宏。
///
/// 从属性化语法定义生成 builtin 注册代码（注册表常量 + 元数据表），
/// 消除手写重复，实现「一处定义，多处使用」。
///
/// ## 语法
///
/// ```ignore
/// define_builtins! {
///     /// 打印值并换行
///     "print" => builtin_print,
///         arity = 0,
///         signature = "print(...) -> nil",
///         desc = "打印值";
///
///     "len" => builtin_len,
///         arity = 1,
///         signature = "len(x) -> int",
///         desc = "返回长度";
/// }
/// ```
///
/// ## 字段说明
///
/// | 字段 | 类型 | 必填 | 说明 |
/// |------|------|------|------|
/// | `arity` | `usize` | **必填** | 必需参数个数 |
/// | `signature` | `str` | 可选 | 函数签名（用于文档/帮助） |
/// | `desc` | `str` | 可选 | 简短描述 |
///
/// ## 向前兼容保证
///
/// - 新增 builtin **只能追加**到末尾（不改变已有顺序）
/// - 新增字段必须有默认值
#[proc_macro]
pub fn define_builtins(input: TokenStream) -> TokenStream {
    nuzo_codegen::builtin_gen::expand_define_builtins(input.into())
        .unwrap_or_else(|e| e.to_compile_error())
        .into()
}
