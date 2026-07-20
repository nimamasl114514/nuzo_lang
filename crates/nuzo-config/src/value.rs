//! 配置值类型 — 动态类型配置值，支持从 TOML/ENV 解析

use crate::error::ConfigError;

/// 动态配置值
#[derive(Debug, Clone)]
pub enum ConfigValue {
    Integer(i64),
    Float(f64),
    Boolean(bool),
    Text(String),
}

impl ConfigValue {
    /// 尝试转为 i64
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            ConfigValue::Integer(n) => Some(*n),
            ConfigValue::Float(f) => Some(*f as i64),
            ConfigValue::Boolean(b) => Some(if *b { 1 } else { 0 }),
            _ => None,
        }
    }

    /// 尝试转为 usize
    pub fn as_usize(&self) -> Option<usize> {
        self.as_i64().and_then(|v| usize::try_from(v).ok())
    }

    /// 尝试转为 u32
    pub fn as_u32(&self) -> Option<u32> {
        self.as_i64().and_then(|v| u32::try_from(v).ok())
    }

    /// 尝试转为 u16
    pub fn as_u16(&self) -> Option<u16> {
        self.as_i64().and_then(|v| u16::try_from(v).ok())
    }

    /// 尝试转为 u8
    pub fn as_u8(&self) -> Option<u8> {
        self.as_i64().and_then(|v| u8::try_from(v).ok())
    }

    /// 尝试转为 f64
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            ConfigValue::Float(f) => Some(*f),
            ConfigValue::Integer(n) => Some(*n as f64),
            _ => None,
        }
    }

    /// 尝试转为 bool
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            ConfigValue::Boolean(b) => Some(*b),
            ConfigValue::Text(s) => match s.as_str() {
                "true" | "1" | "yes" | "on" => Some(true),
                "false" | "0" | "no" | "off" => Some(false),
                _ => None,
            },
            ConfigValue::Integer(n) => Some(*n != 0),
            _ => None,
        }
    }

    /// 尝试转为 &str
    pub fn as_str(&self) -> Option<&str> {
        match self {
            ConfigValue::Text(s) => Some(s),
            _ => None,
        }
    }

    /// 类型名称（用于错误信息）
    pub fn type_name(&self) -> &'static str {
        match self {
            ConfigValue::Integer(_) => "integer",
            ConfigValue::Float(_) => "float",
            ConfigValue::Boolean(_) => "boolean",
            ConfigValue::Text(_) => "string",
        }
    }
}

impl fmt::Display for ConfigValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigValue::Integer(n) => write!(f, "{}", n),
            ConfigValue::Float(n) => write!(f, "{}", n),
            ConfigValue::Boolean(b) => write!(f, "{}", b),
            ConfigValue::Text(s) => write!(f, "{:?}", s),
        }
    }
}

use std::fmt;

/// 从环境变量字符串解析为 ConfigValue
pub fn parse_env_value(raw: &str) -> ConfigValue {
    match raw {
        "true" | "yes" | "on" => return ConfigValue::Boolean(true),
        "false" | "no" | "off" => return ConfigValue::Boolean(false),
        _ => {}
    }
    if let Ok(n) = raw.parse::<i64>() {
        return ConfigValue::Integer(n);
    }
    if let Ok(f) = raw.parse::<f64>() {
        return ConfigValue::Float(f);
    }
    ConfigValue::Text(raw.to_string())
}

/// 从 ConfigValue 提取指定类型，失败则返回 TypeMismatch 错误
pub fn expect_i64(key: &str, val: &ConfigValue) -> Result<i64, ConfigError> {
    val.as_i64().ok_or_else(|| ConfigError::TypeMismatch {
        key: key.to_string(),
        expected: "integer",
        actual: val.to_string(),
    })
}

pub fn expect_usize(key: &str, val: &ConfigValue) -> Result<usize, ConfigError> {
    val.as_usize().ok_or_else(|| ConfigError::TypeMismatch {
        key: key.to_string(),
        expected: "usize",
        actual: val.to_string(),
    })
}

pub fn expect_u32(key: &str, val: &ConfigValue) -> Result<u32, ConfigError> {
    val.as_u32().ok_or_else(|| ConfigError::TypeMismatch {
        key: key.to_string(),
        expected: "u32",
        actual: val.to_string(),
    })
}

pub fn expect_u8(key: &str, val: &ConfigValue) -> Result<u8, ConfigError> {
    val.as_u8().ok_or_else(|| ConfigError::TypeMismatch {
        key: key.to_string(),
        expected: "u8",
        actual: val.to_string(),
    })
}

pub fn expect_f64(key: &str, val: &ConfigValue) -> Result<f64, ConfigError> {
    val.as_f64().ok_or_else(|| ConfigError::TypeMismatch {
        key: key.to_string(),
        expected: "float",
        actual: val.to_string(),
    })
}

pub fn expect_bool(key: &str, val: &ConfigValue) -> Result<bool, ConfigError> {
    val.as_bool().ok_or_else(|| ConfigError::TypeMismatch {
        key: key.to_string(),
        expected: "boolean",
        actual: val.to_string(),
    })
}
