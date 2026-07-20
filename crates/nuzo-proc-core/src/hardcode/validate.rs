//! # 常量校验规则引擎
//!
//! 提供对 `define_constants!` 注册的常量进行运行时校验的能力：
//!
//! - **类型校验**：环境变量覆盖值能否解析为目标类型
//! - **范围校验**：数值常量是否落在合理区间（如 `MAX_STACK_SIZE > 0`）
//! - **自定义规则**：用户可通过 [`register_rule`] 注册任意校验逻辑
//!
//! ## 校验流程
//!
//! 1. 遍历注册表中的所有常量
//! 2. 对每个常量，检查是否有环境变量覆盖
//! 3. 若有覆盖，尝试解析为目标类型；解析失败则记录错误
//! 4. 对每个常量，运行所有已注册的自定义规则
//! 5. 返回 [`ValidationResult`] 列表
//!
//! # Feature Gate
//!
//! 本模块依赖 `env-override` feature（用于读取环境变量覆盖）。

#![cfg(feature = "env-override")]

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use super::env::EnvOverride;
use super::registry;
use super::types::ConstantInfo;

/// 校验结果。
#[derive(Debug, Clone)]
pub struct ValidationResult {
    /// 常量名
    pub name: String,
    /// 是否通过校验
    pub valid: bool,
    /// 错误信息列表（`valid == false` 时非空）
    pub errors: Vec<String>,
}

impl ValidationResult {
    /// 创建一个通过校验的结果。
    pub fn ok(name: &str) -> Self {
        Self { name: name.to_string(), valid: true, errors: Vec::new() }
    }

    /// 创建一个校验失败的结果。
    pub fn fail(name: &str, errors: Vec<String>) -> Self {
        Self { name: name.to_string(), valid: false, errors }
    }

    /// 返回错误数量。
    pub fn error_count(&self) -> usize {
        self.errors.len()
    }
}

/// 自定义校验规则签名。
///
/// 接收常量元数据，返回 `Ok(())` 表示通过，`Err(message)` 表示失败。
pub type Rule = fn(&ConstantInfo) -> Result<(), String>;

/// 规则存储。
struct RuleStore {
    /// 常量名 -> 规则列表
    rules: HashMap<String, Vec<Rule>>,
}

impl RuleStore {
    fn new() -> Self {
        Self { rules: HashMap::new() }
    }
}

/// 全局规则存储（线程安全）。
static RULES: LazyLock<Mutex<RuleStore>> = LazyLock::new(|| Mutex::new(RuleStore::new()));

/// 获取 RULES 锁，poisoned 时恢复（不 panic）。
///
/// 采用 poison recovery 模式（`into_inner()` 取出内部数据），
/// 避免因其它线程 panic 导致整个进程不可用。
fn lock_rules() -> std::sync::MutexGuard<'static, RuleStore> {
    RULES.lock().unwrap_or_else(|e| e.into_inner())
}

/// 为指定常量注册一条校验规则。
///
/// # 示例
///
/// ```no_run
/// use nuzo_proc_core::hardcode::validate::register_rule;
/// use nuzo_proc_core::hardcode::types::ConstantInfo;
///
/// register_rule("DEFAULT_MAX_STACK_SIZE", |info| {
///     let v = info.parse_as_i64().unwrap_or(0);
///     if v < 1024 {
///         Err(format!("stack size too small: {} < 1024", v))
///     } else {
///         Ok(())
///     }
/// });
/// ```
pub fn register_rule(name: &str, rule: Rule) {
    let mut guard = lock_rules();
    guard.rules.entry(name.to_string()).or_default().push(rule);
}

/// 校验所有已注册常量。
///
/// 返回每个常量的校验结果（按注册顺序）。
pub fn validate_all() -> Vec<ValidationResult> {
    let all = registry::all();
    let rules_guard = lock_rules();
    let mut results = Vec::with_capacity(all.len());

    for info in all {
        let mut errors = Vec::new();

        // 1. 检查环境变量覆盖是否能正确解析
        if let Some(_override) = EnvOverride::get(info.name) {
            // 尝试解析为目标类型
            if info.is_integer() {
                if EnvOverride::get_as_i64(info.name).is_none() {
                    errors.push(format!("env override for {} cannot be parsed as i64", info.name));
                }
            } else if info.is_float() {
                if EnvOverride::get_as_f64(info.name).is_none() {
                    errors.push(format!("env override for {} cannot be parsed as f64", info.name));
                }
            } else if info.is_bool() && EnvOverride::get_as_bool(info.name).is_none() {
                errors.push(format!("env override for {} cannot be parsed as bool", info.name));
            }
        }

        // 2. 运行自定义规则
        if let Some(rules) = rules_guard.rules.get(info.name) {
            for rule in rules {
                if let Err(msg) = rule(&info) {
                    errors.push(msg);
                }
            }
        }

        if errors.is_empty() {
            results.push(ValidationResult::ok(info.name));
        } else {
            results.push(ValidationResult::fail(info.name, errors));
        }
    }

    results
}

/// 校验单个常量。
///
/// 若常量未注册，返回 `None`。
pub fn validate(name: &str) -> Option<ValidationResult> {
    let info = registry::get(name)?;
    let rules_guard = lock_rules();
    let mut errors = Vec::new();

    // 1. 检查环境变量覆盖
    if let Some(_override) = EnvOverride::get(info.name) {
        if info.is_integer() {
            if EnvOverride::get_as_i64(info.name).is_none() {
                errors.push(format!("env override for {} cannot be parsed as i64", info.name));
            }
        } else if info.is_float() {
            if EnvOverride::get_as_f64(info.name).is_none() {
                errors.push(format!("env override for {} cannot be parsed as f64", info.name));
            }
        } else if info.is_bool() && EnvOverride::get_as_bool(info.name).is_none() {
            errors.push(format!("env override for {} cannot be parsed as bool", info.name));
        }
    }

    // 2. 运行自定义规则
    if let Some(rules) = rules_guard.rules.get(info.name) {
        for rule in rules {
            if let Err(msg) = rule(&info) {
                errors.push(msg);
            }
        }
    }

    if errors.is_empty() {
        Some(ValidationResult::ok(info.name))
    } else {
        Some(ValidationResult::fail(info.name, errors))
    }
}

/// 清空所有已注册规则（仅测试用）。
#[cfg(feature = "test-utils")]
pub fn clear_rules() {
    let mut guard = lock_rules();
    guard.rules.clear();
}

/// 返回已注册规则的数量。
pub fn rule_count() -> usize {
    let guard = lock_rules();
    guard.rules.values().map(|v| v.len()).sum()
}
