//! # Nuzo Helpers — Nuzo 内置函数辅助库
//!
//! **层级**: L3（编译器层 / 运行时辅助层）—— 集中管理所有 Builtin 函数注册表，为 VM 提供 I/O、类型转换、字符串、数组、数学、时间等运行时函数。
//!
//! **主要入口**: [`BuiltinRegistry`], [`BuiltinFn`], [`builtin_names`]
//!
//! ## 模块职责
//!
//! 本库采用**模块化注册架构**，所有内置函数通过 [`BuiltinRegistry`](builtins::BuiltinRegistry) 统一管理。
//!
//! | 模块 | 功能域 | 注册入口 |
//! |------|--------|----------|
//! | [`builtins`] | 核心 I/O + 类型断言 + 集合操作 | [`BuiltinRegistry::new()`](builtins::BuiltinRegistry::new) |
//! | [`convert`] | 类型转换 (`int`, `float`, `bool`, `num`, `is_*`) | [`convert::register()`](convert::register) |
//! | [`string`] | 字符串操作 (`split`, `join`, `trim`, `upper`, `lower`...) | [`string::register()`](string::register) |
//! | [`array`] | 数组操作 (`index_of`, `slice`, `sort`, `concat`...) | [`array::register()`](array::register) |
//! | [`math`] | 数学运算 (`abs`, `sqrt`, `pow`, `random`, `sin`...) | [`math::register()`](math::register) |
//! | [`io`] | 文件 I/O (`read_file`, `write_file`, `input`) | [`io::register()`](io::register) |
//! | [`time`] | 时间处理 (`now`, `sleep`, `timestamp`, `clock`) | [`time::register()`](time::register) |
//! | [`debug`] | 调试工具 (`dump`, `format`, `time`, `time_end`) | [`debug::register()`](debug::register) |
//! | [`validation`] | 参数校验工具 | (内部) |
//!
//! ## 开发者速查：常见任务 → 代码位置
//!
//! | 任务 | 位置 |
//! |------|------|
//! | "加新 builtin 函数" | 在对应 domain.rs 实现 fn → 调用 `registry.register("name", fn, arity)` |
//! | "改函数参数校验" | 对应 domain.rs 函数入口的参数校验逻辑 |
//! | "改 BuiltinRegistry 行为" | `builtins.rs: BuiltinRegistry` struct |

#![allow(clippy::result_large_err)]

// Crate 元数据——外层属性形式（`#![inner_attr]` 在 stable Rust 不稳定）
// 层级 L4：与 DEVELOPMENT.md 中 "frontend / bytecode / helpers" 同层（L4）保持一致，
// 为 VM (L6) 与 compiler (L6) 提供运行时辅助。
#[nuzo_proc::crate_meta(layer = 4, description = "内置函数注册表", entry_type = "BuiltinRegistry")]
const _NUZO_CRATE_META_ANCHOR: () = ();

// validation 必须首位声明并加 #[macro_use]：
// 其内定义的 require_arg_count! / require_number! / define_builtin_impl! 等宏
// 需对后续所有子模块可见（math.rs / convert.rs 等使用 define_builtin_impl! 时依赖）。
#[macro_use]
pub mod validation;

pub mod array;
pub mod builtins;
pub mod convert;
pub mod debug;
pub mod io;
pub mod math;
pub mod string;
pub mod sys;
pub mod time;

// --- 核心注册表（VM 通过此类型发现所有 builtin）---
pub use builtins::BuiltinRegistry;

// ── builtins 模块导出（逐条显式，禁止 glob re-export）───────────
// 核心注册表类型（VM/CLI/API 通过 nuzo_helpers::BuiltinRegistry 引用）
pub use builtins::BuiltinFn;
// 信号与输出捕获工具函数
pub use builtins::BUILTIN_CALLED_KEY;
pub use builtins::OutputCaptureGuard;
pub use builtins::configure_output_capture;

/// 返回所有已注册内置函数的名称列表（便利函数）
///
/// 等价于 `BuiltinRegistry::new().names()`，供编译器在 IR 构建阶段
/// 识别全局内置函数，避免将其误判为闭包捕获变量。
///
/// # 设计动机
///
/// 原实现将内置函数名硬编码在 `nuzo_ir::builder::is_global_function` 中（约 60 个名字），
/// 每次新增/重命名 builtin 都需同步修改 IR 层，违反依赖倒置原则。
/// 现由调用方（编译器）通过此函数获取权威列表并注入 IR 构建器。
///
/// # 性能说明
///
/// 每次调用会构造新的注册表（O(n) 注册 + O(n) 收集名称）。
/// 仅在编译入口调用一次，开销可忽略；如需高频调用，请直接持有 `BuiltinRegistry` 实例。
///
/// # 示例
///
/// ```rust,ignore
/// let names = nuzo_helpers::builtin_names();
/// assert!(names.contains(&"println"));
/// ```
pub fn builtin_names() -> Vec<&'static str> {
    BuiltinRegistry::new().names()
}
