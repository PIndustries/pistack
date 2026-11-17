// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The `Config` tree and the loader that builds it from a TOML file and
//! `PISTACK__*` environment overrides.

mod config_error;
mod config_load;
mod constants;
mod example_toml;
mod log_config;
mod log_format;
mod server_config;
#[cfg(test)]
mod tests;
mod tls_config;
mod tls_paths;

pub use config_error::ConfigError;
pub use config_load::Config;
pub use constants::DEFAULT_PATHS;
pub use constants::ENV_CONFIG_PATH;
pub use constants::ENV_PREFIX;
pub use example_toml::EXAMPLE_TOML;
pub use log_config::LogConfig;
pub use log_format::LogFormat;
pub use server_config::ServerConfig;
pub use tls_config::TlsConfig;
pub use tls_paths::TlsPaths;
