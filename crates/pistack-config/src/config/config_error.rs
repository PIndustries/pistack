// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `ConfigError`: why configuration could not be loaded.

use std::path::PathBuf;

/// Why configuration could not be loaded.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("config file {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("config file {path}: {message}")]
    Parse { path: PathBuf, message: String },
    #[error("environment variable {var}: {message}")]
    Env { var: String, message: String },
    #[error("config: {0}")]
    Invalid(String),
    #[error("module section [modules.{module}]: {message}")]
    Module { module: String, message: String },
}
