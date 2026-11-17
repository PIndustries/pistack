// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Configuration loading: a TOML file, then environment overrides.
//!
//! Precedence, lowest to highest: built-in defaults, the TOML file,
//! environment variables. Command-line flags are the binary's business;
//! it applies them on top of the returned [`Config`].
//!
//! Environment variables are `PISTACK__` followed by the key path with
//! `__` between segments, upper-case:
//!
//! ```text
//! PISTACK__SERVER__LISTEN=0.0.0.0:8443       -> server.listen
//! PISTACK__DATA_DIR=/srv/pistack             -> data_dir
//! PISTACK__TLS__CERT=/etc/pistack/tls.crt    -> tls.cert
//! PISTACK__MODULES__FORGE__SCHEDULER=spread  -> modules.forge.scheduler
//! ```
//!
//! A value is parsed as a TOML value when it is one (`8443`, `true`,
//! `["a","b"]`, `"quoted"`), otherwise taken as a string. When the key
//! already holds a string the value stays a string, so
//! `PISTACK__SERVER__PUBLIC_URL=true` does not turn a URL into a boolean.
//!
//! Module sections live under `[modules.<name>]` and are kept as raw
//! [`toml::Table`]s; each module deserialises its own with
//! [`Config::module`].

mod config;

pub use config::*;
