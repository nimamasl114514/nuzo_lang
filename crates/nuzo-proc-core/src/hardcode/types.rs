//! # 常量元数据类型
//!
//! 提供 [`ConstantInfo`] 结构体，用于在运行时描述一个由 `define_constants!` 宏
//! 注册的编译期常量。所有字段均为 `&'static str`，零分配、零拷贝。

/// 常量元数据。
///
/// 由 `define_constants!` 宏在编译期生成并注册到 [`crate::hardcode::registry`]，
/// 用于运行时自省（introspection）、JSON 导出与校验。
///
/// # 字段说明
///
/// | 字段 | 含义 |
/// |------|------|
/// | `name` | 常量名（如 `"DEFAULT_MAX_STACK_SIZE"`）|
/// | `type_name` | 类型名（如 `"usize"`、`"f64"`、`"&str"`）|
/// | `value_str` | 值的字符串形式（如 `"65536"`、`"0.5"`、`"<source>"`）|
/// | `doc` | 文档注释原文（含 `///` 前缀已剥离），可为空 |
/// | `module_path` | 注册时所在模块路径（如 `"nuzo_core::constants"`）|
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConstantInfo {
    /// 常量名
    pub name: &'static str,
    /// 类型名（Rust 类型）
    pub type_name: &'static str,
    /// 值的字符串形式
    pub value_str: &'static str,
    /// 文档注释（已剥离 `///` 前缀），可为空字符串
    pub doc: &'static str,
    /// 注册时所在模块路径（`module_path!()` 的结果）
    pub module_path: &'static str,
}

impl ConstantInfo {
    /// 判断是否为整数类型（`u8`/`u16`/`u32`/`u64`/`usize`/`i8`/`i16`/`i32`/`i64`/`isize`）。
    pub fn is_integer(&self) -> bool {
        matches!(
            self.type_name,
            "u8" | "u16" | "u32" | "u64" | "usize" | "i8" | "i16" | "i32" | "i64" | "isize"
        )
    }

    /// 判断是否为浮点类型（`f32`/`f64`）。
    pub fn is_float(&self) -> bool {
        matches!(self.type_name, "f32" | "f64")
    }

    /// 判断是否为布尔类型（`bool`）。
    pub fn is_bool(&self) -> bool {
        self.type_name == "bool"
    }

    /// 判断是否为字符串类型（`&str`）。
    pub fn is_string(&self) -> bool {
        self.type_name == "&str" || self.type_name == "&'static str"
    }

    /// 尝试将 `value_str` 解析为 `i64`。
    ///
    /// 仅对整数类型有意义；浮点/字符串类型返回 `None`。
    pub fn parse_as_i64(&self) -> Option<i64> {
        if !self.is_integer() {
            return None;
        }
        self.value_str.parse::<i64>().ok()
    }

    /// 尝试将 `value_str` 解析为 `f64`。
    ///
    /// 对整数与浮点类型均有效；字符串/布尔类型返回 `None`。
    pub fn parse_as_f64(&self) -> Option<f64> {
        if self.is_integer() || self.is_float() { self.value_str.parse::<f64>().ok() } else { None }
    }

    /// 返回文档注释的非空文本（若 `doc` 为空则返回 `None`）。
    pub fn doc_text(&self) -> Option<&'static str> {
        if self.doc.is_empty() { None } else { Some(self.doc) }
    }
}
