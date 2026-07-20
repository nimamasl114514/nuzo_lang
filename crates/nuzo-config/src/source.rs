//! 配置源 — TOML 文件 + 环境变量

use crate::error::{ConfigError, ConfigResult};
use crate::toml_parser::TomlTable;
use crate::value::{ConfigValue, parse_env_value};

/// 配置源：从不同来源收集的 key → value 映射
#[derive(Debug, Clone, Default)]
pub struct ConfigSource {
    table: TomlTable,
}

impl ConfigSource {
    /// 创建空配置源
    pub fn new() -> Self {
        Self::default()
    }

    /// 从 TOML 字符串加载
    pub fn from_toml_str(input: &str) -> ConfigResult<Self> {
        let table = TomlTable::parse(input)?;
        Ok(Self { table })
    }

    /// 从 TOML 文件加载
    ///
    /// 文件不存在时返回空源（不报错），符合"可选配置"语义。
    pub fn from_toml_file(path: &std::path::Path) -> ConfigResult<Self> {
        match std::fs::read_to_string(path) {
            Ok(content) => Self::from_toml_str(&content),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::new()),
            Err(e) => Err(ConfigError::Io(format!("cannot read {:?}: {}", path, e))),
        }
    }

    /// 从环境变量加载
    ///
    /// 识别 `NUZO_` 前缀的环境变量，将 `NUZO_SECTION_KEY` 映射为 `section.key`。
    ///
    /// 示例：
    /// - `NUZO_GC_THRESHOLD=20000000` → `gc.threshold = 20000000`
    /// - `NUZO_VM_MAX_STACK_SIZE=131072` → `vm.max_stack_size = 131072`
    /// - `NUZO_ARENA_ENABLED=false` → `arena.enabled = false`
    pub fn from_env() -> Self {
        let mut source = Self::new();
        const PREFIX: &str = "NUZO_";

        for (key, val) in std::env::vars() {
            if let Some(suffix) = key.strip_prefix(PREFIX) {
                // NUZO_GC_THRESHOLD → gc.threshold
                let config_key = env_to_config_key(suffix);
                let value = parse_env_value(&val);
                source.table.insert(&config_key, value);
            }
        }

        source
    }

    /// 获取值
    pub fn get(&self, key: &str) -> Option<&ConfigValue> {
        self.table.get(key)
    }

    /// 合并另一个源（other 覆盖 self）
    pub fn merge(&mut self, other: &ConfigSource) {
        self.table.merge(&other.table);
    }

    /// 内部 table 引用
    pub fn as_table(&self) -> &TomlTable {
        &self.table
    }
}

/// 将环境变量后缀转为配置键
///
/// 规则：
/// - 第一个**单下划线** `_` 视为 section/key 分隔符，转为 `.`。
///   例如 `GC_THRESHOLD` → `gc.threshold`，`ARENA_ENABLED` → `arena.enabled`。
/// - 后续单下划线保留为 `_`，以匹配 TOML 配置中的字段名
///   （如 `vm.max_stack_size`，而非 `vm.max.stack.size`）。
///   例如 `VM_MAX_STACK_SIZE` → `vm.max_stack_size`，
///   `GC_SURVIVAL_RATIO_THRESHOLD` → `gc.survival_ratio_threshold`。
/// - 双下划线 `__` 视为字面下划线，不作为分隔符
///   （用于 section/key 名本身包含下划线的场景）。
///   例如 `VM__SPECIAL_KEY` → `vm_special.key`。
fn env_to_config_key(suffix: &str) -> String {
    let mut result = String::with_capacity(suffix.len());
    let mut chars = suffix.chars().peekable();
    let mut separator_seen = false;

    while let Some(ch) = chars.next() {
        if ch == '_' {
            if chars.peek() == Some(&'_') {
                // 双下划线：转为字面下划线（不作为分隔符）
                result.push('_');
                chars.next();
            } else if !separator_seen {
                // 第一个单下划线分隔符：转为 `.`
                result.push('.');
                separator_seen = true;
            } else {
                // 后续单下划线：保留为字面下划线
                result.push('_');
            }
        } else if ch.is_uppercase() {
            result.push(ch.to_ascii_lowercase());
        } else {
            result.push(ch);
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_to_config_key_basic() {
        assert_eq!(env_to_config_key("GC_THRESHOLD"), "gc.threshold");
        // 第一个单下划线作为 section/key 分隔符，后续单下划线保留以匹配 TOML 字段名
        assert_eq!(env_to_config_key("VM_MAX_STACK_SIZE"), "vm.max_stack_size");
        assert_eq!(env_to_config_key("ARENA_ENABLED"), "arena.enabled");
        assert_eq!(env_to_config_key("GC_SURVIVAL_RATIO_THRESHOLD"), "gc.survival_ratio_threshold");
        assert_eq!(
            env_to_config_key("VM_DIAGNOSTIC_REGISTER_WINDOW"),
            "vm.diagnostic_register_window"
        );
    }

    #[test]
    fn env_to_config_key_double_underscore() {
        // NUZO_VM__SPECIAL_KEY → vm_special.key（双下划线=原始下划线）
        assert_eq!(env_to_config_key("VM__SPECIAL_KEY"), "vm_special.key");
    }

    #[test]
    fn from_toml_str() {
        let src = ConfigSource::from_toml_str("[vm]\nmax_stack_size = 999").unwrap();
        assert_eq!(src.get("vm.max_stack_size").unwrap().as_usize(), Some(999));
    }

    #[test]
    fn from_missing_file() {
        let src =
            ConfigSource::from_toml_file(std::path::Path::new("/nonexistent/nuzo.toml")).unwrap();
        assert!(src.get("anything").is_none());
    }

    #[test]
    fn merge_sources() {
        let mut a = ConfigSource::from_toml_str("key = 1").unwrap();
        let b = ConfigSource::from_toml_str("key = 2\nother = 3").unwrap();
        a.merge(&b);
        assert_eq!(a.get("key").unwrap().as_i64(), Some(2));
        assert_eq!(a.get("other").unwrap().as_i64(), Some(3));
    }
}
