//! # 环境变量覆盖机制
//!
//! 允许通过环境变量在**运行时**覆盖 `define_constants!` 注册的编译期常量值。
//!
//! ## 命名规则
//!
//! 环境变量名 = `NUZO_` + 常量名（全大写）。
//!
//! | 常量名 | 环境变量 |
//! |--------|---------|
//! | `DEFAULT_MAX_STACK_SIZE` | `NUZO_DEFAULT_MAX_STACK_SIZE` |
//! | `GC_DEFAULT_THRESHOLD` | `NUZO_GC_DEFAULT_THRESHOLD` |
//!
//! ## 类型转换
//!
//! - 整数类型：`EnvOverride::get_as_i64`
//! - 浮点类型：`EnvOverride::get_as_f64`
//! - 布尔类型：`EnvOverride::get_as_bool`（接受 `1`/`0`/`true`/`false`/`yes`/`no`）
//! - 字符串类型：`EnvOverride::get`（原样返回）
//!
//! ## 示例
//!
//! ```no_run
//! use nuzo_proc_core::hardcode::env::EnvOverride;
//!
//! // 读取覆盖值，若无则使用默认值
//! let stack_size = EnvOverride::get_as_i64("DEFAULT_MAX_STACK_SIZE")
//!     .map(|v| v as usize)
//!     .unwrap_or(65536);
//! ```
//!
//! # Feature Gate
//!
//! 本模块仅在 `env-override` feature 启用时可用。

#![cfg(feature = "env-override")]

use std::env;

/// 环境变量覆盖入口。
///
/// 所有方法均为关联函数（无需实例化），直接通过 `EnvOverride::get(name)` 调用。
pub struct EnvOverride;

impl EnvOverride {
    /// 读取字符串形式的覆盖值。
    ///
    /// 若环境变量未设置或为空字符串，返回 `None`。
    pub fn get(name: &str) -> Option<String> {
        let key = Self::env_key(name);
        match env::var(&key) {
            Ok(v) if !v.is_empty() => Some(v),
            _ => None,
        }
    }

    /// 读取并解析为 `i64`。
    pub fn get_as_i64(name: &str) -> Option<i64> {
        Self::get(name)?.parse::<i64>().ok()
    }

    /// 读取并解析为 `f64`。
    pub fn get_as_f64(name: &str) -> Option<f64> {
        Self::get(name)?.parse::<f64>().ok()
    }

    /// 读取并解析为 `bool`。
    ///
    /// 接受（不区分大小写）：`1`/`true`/`yes`/`on` → `true`；
    /// `0`/`false`/`no`/`off` → `false`；其他返回 `None`。
    pub fn get_as_bool(name: &str) -> Option<bool> {
        let raw = Self::get(name)?;
        match raw.to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => Some(true),
            "0" | "false" | "no" | "off" => Some(false),
            _ => None,
        }
    }

    /// 构造环境变量名：`NUZO_` + 常量名（大写）。
    pub fn env_key(name: &str) -> String {
        format!("NUZO_{}", name.to_ascii_uppercase())
    }

    /// 列出所有已设置的 `NUZO_*` 环境变量。
    ///
    /// 用于诊断与调试。
    pub fn list_overrides() -> Vec<String> {
        env::vars()
            .filter_map(|(k, _)| if k.starts_with("NUZO_") { Some(k) } else { None })
            .collect()
    }
}

/// 针对单个常量的覆盖读取器（Builder 风格）。
///
/// 相比 `EnvOverride` 的关联函数，`OverrideReader` 缓存常量名，
/// 便于链式调用：
///
/// ```no_run
/// use nuzo_proc_core::hardcode::env::OverrideReader;
///
/// let val = OverrideReader::new("DEFAULT_MAX_STACK_SIZE")
///     .as_i64()
///     .or(Some(65536));
/// ```
pub struct OverrideReader {
    name: String,
}

impl OverrideReader {
    /// 创建一个新的 `OverrideReader`。
    pub fn new(name: &str) -> Self {
        Self { name: name.to_string() }
    }

    /// 读取原始字符串值。
    pub fn raw(&self) -> Option<String> {
        EnvOverride::get(&self.name)
    }

    /// 读取并解析为 `i64`。
    pub fn as_i64(&self) -> Option<i64> {
        EnvOverride::get_as_i64(&self.name)
    }

    /// 读取并解析为 `f64`。
    pub fn as_f64(&self) -> Option<f64> {
        EnvOverride::get_as_f64(&self.name)
    }

    /// 读取并解析为 `bool`。
    pub fn as_bool(&self) -> Option<bool> {
        EnvOverride::get_as_bool(&self.name)
    }

    /// 读取字符串值（`raw` 的别名，语义更清晰）。
    pub fn as_str(&self) -> Option<String> {
        self.raw()
    }

    /// 若覆盖存在则返回覆盖值，否则返回 `default`。
    ///
    /// # 类型约束
    ///
    /// `T` 必须能从 `Option<String>` 转换而来。对于基本类型，
    /// 建议直接使用 `as_i64` / `as_f64` / `as_bool` 等方法。
    pub fn or_string(self, default: &str) -> String {
        self.raw().unwrap_or_else(|| default.to_string())
    }
}
