//! 配置错误类型

use std::fmt;

/// 配置错误
#[derive(Debug)]
pub enum ConfigError {
    /// TOML 解析错误
    TomlParse { line: usize, message: String },
    /// 类型转换错误
    TypeMismatch { key: String, expected: &'static str, actual: String },
    /// IO 错误
    Io(String),
    /// 环境变量解析错误
    EnvParse { key: String, value: String, message: String },
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::TomlParse { line, message } => {
                write!(f, "TOML parse error at line {}: {}", line, message)
            }
            ConfigError::TypeMismatch { key, expected, actual } => {
                write!(f, "type mismatch for '{}': expected {}, got {}", key, expected, actual)
            }
            ConfigError::Io(msg) => write!(f, "IO error: {}", msg),
            ConfigError::EnvParse { key, value, message } => {
                write!(f, "env var NUZO_{}={:?}: {}", key, value, message)
            }
        }
    }
}

impl std::error::Error for ConfigError {}

/// 配置结果
pub type ConfigResult<T> = Result<T, ConfigError>;
