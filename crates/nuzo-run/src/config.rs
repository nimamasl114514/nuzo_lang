//! Config loading utilities.

use crate::error::{NuzoResult, internal_err};
use nuzo_config::Config;
use std::path::Path;

/// Load a Config from a TOML file.
pub fn load_config_file(path: impl AsRef<Path>) -> NuzoResult<Config> {
    Config::from_toml_file(path.as_ref())
        .map_err(|e| internal_err(format!("Config load error: {}", e)))
}

/// Load config from environment variables and default config file paths.
pub fn load_env_config() -> NuzoResult<Config> {
    Ok(Config::from_env())
}
