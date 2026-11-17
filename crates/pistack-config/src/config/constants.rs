// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The environment variable names and default file paths config loading uses.

/// Prefix every override variable starts with.
pub const ENV_PREFIX: &str = "PISTACK__";
/// Variable naming the config file when `Config::load(None)` is called.
pub const ENV_CONFIG_PATH: &str = "PISTACK_CONFIG";
/// Paths tried, in order, when no file is given and `PISTACK_CONFIG` is unset.
pub const DEFAULT_PATHS: &[&str] = &["pistack.toml", "/etc/pistack/pistack.toml"];
