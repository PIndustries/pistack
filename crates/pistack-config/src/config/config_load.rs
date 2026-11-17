// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `Config`: the top-level configuration and how it is loaded.
//!
//! Loading, the private table helpers it drives (defaults, merge,
//! environment overrides, type coercion) all live here because they exist
//! only to build a `Config`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use toml::{Table, Value};

use super::config_error::ConfigError;
use super::constants::{DEFAULT_PATHS, ENV_CONFIG_PATH, ENV_PREFIX};
use super::log_config::LogConfig;
use super::server_config::ServerConfig;
use super::tls_config::TlsConfig;
use super::tls_paths::TlsPaths;

/// Top-level configuration.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub data_dir: PathBuf,
    /// Node name reported to peers and agents; empty means the hostname.
    pub node_name: String,
    pub server: ServerConfig,
    pub tls: TlsConfig,
    pub log: LogConfig,
    /// Raw per-module sections, keyed by module name.
    pub modules: BTreeMap<String, Table>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            data_dir: PathBuf::from("/var/lib/pistack"),
            node_name: String::new(),
            server: ServerConfig::default(),
            tls: TlsConfig::default(),
            log: LogConfig::default(),
            modules: BTreeMap::new(),
        }
    }
}

impl Config {
    /// Load configuration.
    ///
    /// * `Some(path)`: that file must exist and parse.
    /// * `None`: `$PISTACK_CONFIG` if set (must exist), else the first of
    ///   [`DEFAULT_PATHS`] that exists, else defaults only.
    ///
    /// Then `PISTACK__*` variables from the process environment are applied.
    pub fn load(path: Option<&Path>) -> Result<Self, ConfigError> {
        let env: Vec<(String, String)> = std::env::vars().collect();
        Self::load_with_env(path, env)
    }

    /// [`Config::load`] with an explicit environment, for tests and for
    /// callers that want to control exactly which variables apply. The
    /// `PISTACK_CONFIG` lookup also comes from `env`.
    pub fn load_with_env<I, K, V>(path: Option<&Path>, env: I) -> Result<Self, ConfigError>
    where
        I: IntoIterator<Item = (K, V)>,
        K: AsRef<str>,
        V: AsRef<str>,
    {
        let env: Vec<(String, String)> = env
            .into_iter()
            .map(|(k, v)| (k.as_ref().to_owned(), v.as_ref().to_owned()))
            .collect();

        let path: Option<PathBuf> = match path {
            Some(p) => Some(p.to_path_buf()),
            None => {
                let from_env = env
                    .iter()
                    .find(|(k, _)| k == ENV_CONFIG_PATH)
                    .map(|(_, v)| PathBuf::from(v))
                    .filter(|p| !p.as_os_str().is_empty());
                match from_env {
                    Some(p) => Some(p),
                    None => DEFAULT_PATHS
                        .iter()
                        .map(PathBuf::from)
                        .find(|p| p.is_file()),
                }
            }
        };

        let file_table = match &path {
            Some(p) => {
                let text = std::fs::read_to_string(p).map_err(|source| ConfigError::Read {
                    path: p.clone(),
                    source,
                })?;
                tracing::debug!(path = %p.display(), "loading config file");
                text.parse::<Table>().map_err(|e| ConfigError::Parse {
                    path: p.clone(),
                    message: e.to_string(),
                })?
            }
            None => {
                tracing::debug!("no config file found; using defaults");
                Table::new()
            }
        };

        // Seed with the defaults so environment overrides know the type of
        // every known key, then lay the file over them.
        let mut table = defaults_table()?;
        merge_tables(&mut table, file_table);
        apply_env(
            &mut table,
            env.iter().map(|(k, v)| (k.as_str(), v.as_str())),
        )?;
        Self::from_table(table)
    }

    /// Parse a TOML document without touching the filesystem or environment.
    pub fn from_toml(text: &str) -> Result<Self, ConfigError> {
        let table = text
            .parse::<Table>()
            .map_err(|e| ConfigError::Invalid(e.to_string()))?;
        Self::from_table(table)
    }

    fn from_table(table: Table) -> Result<Self, ConfigError> {
        let cfg: Config = Value::Table(table)
            .try_into()
            .map_err(|e: toml::de::Error| ConfigError::Invalid(e.to_string()))?;
        cfg.validate()?;
        Ok(cfg)
    }

    fn validate(&self) -> Result<(), ConfigError> {
        if self.server.listen.is_empty() {
            return Err(ConfigError::Invalid(
                "server.listen must not be empty".into(),
            ));
        }
        if self.server.listen.rsplit_once(':').is_none() {
            return Err(ConfigError::Invalid(format!(
                "server.listen {:?} must be host:port",
                self.server.listen
            )));
        }
        if self.tls.cert.is_some() != self.tls.key.is_some() {
            return Err(ConfigError::Invalid(
                "tls.cert and tls.key must be set together".into(),
            ));
        }
        if self.server.long_poll_secs == 0 {
            return Err(ConfigError::Invalid(
                "server.long_poll_secs must be at least 1".into(),
            ));
        }
        Ok(())
    }

    /// Deserialise the `[modules.<name>]` section into `T`. A missing
    /// section yields `T::default()`; a present one must deserialise fully.
    pub fn module<T: DeserializeOwned + Default>(&self, name: &str) -> Result<T, ConfigError> {
        match self.modules.get(name) {
            None => Ok(T::default()),
            Some(table) => Value::Table(table.clone())
                .try_into()
                .map_err(|e: toml::de::Error| ConfigError::Module {
                    module: name.to_owned(),
                    message: e.to_string(),
                }),
        }
    }

    /// The base URL clients should use: `server.public_url` if set, else
    /// derived from `server.listen` (an unspecified bind address becomes
    /// `localhost`).
    pub fn public_url(&self) -> String {
        if !self.server.public_url.is_empty() {
            return self.server.public_url.trim_end_matches('/').to_owned();
        }
        let scheme = if self.server.insecure_http {
            "http"
        } else {
            "https"
        };
        let (host, port) = self
            .server
            .listen
            .rsplit_once(':')
            .unwrap_or((&self.server.listen, "8443"));
        let host = match host {
            "" | "0.0.0.0" | "[::]" | "::" => "localhost",
            h => h,
        };
        format!("{scheme}://{host}:{port}")
    }

    /// Where TLS material lives: configured paths, or the defaults under
    /// `<data_dir>/tls`.
    pub fn tls_paths(&self) -> TlsPaths {
        let dir = self.data_dir.join("tls");
        TlsPaths {
            cert: self
                .tls
                .cert
                .clone()
                .unwrap_or_else(|| dir.join("server.crt")),
            key: self
                .tls
                .key
                .clone()
                .unwrap_or_else(|| dir.join("server.key")),
            ca: self.tls.ca.clone().unwrap_or_else(|| dir.join("ca.crt")),
        }
    }

    /// Render as TOML (round-trips through [`Config::from_toml`]).
    pub fn to_toml(&self) -> Result<String, ConfigError> {
        toml::to_string_pretty(self).map_err(|e| ConfigError::Invalid(e.to_string()))
    }
}

fn defaults_table() -> Result<Table, ConfigError> {
    match Value::try_from(Config::default()) {
        Ok(Value::Table(t)) => Ok(t),
        Ok(_) => Err(ConfigError::Invalid(
            "defaults did not serialise to a table".into(),
        )),
        Err(e) => Err(ConfigError::Invalid(format!(
            "defaults did not serialise: {e}"
        ))),
    }
}

/// Deep-merge `overlay` into `base`: tables recurse, anything else replaces.
fn merge_tables(base: &mut Table, overlay: Table) {
    for (k, v) in overlay {
        match (base.get_mut(&k), v) {
            (Some(Value::Table(b)), Value::Table(o)) => merge_tables(b, o),
            (_, v) => {
                base.insert(k, v);
            }
        }
    }
}

/// Apply `PISTACK__A__B=value` overrides to `table`.
fn apply_env<'a>(
    table: &mut Table,
    env: impl Iterator<Item = (&'a str, &'a str)>,
) -> Result<(), ConfigError> {
    let mut vars: Vec<(&str, &str)> = env
        .filter(|(k, _)| k.starts_with(ENV_PREFIX) && k.len() > ENV_PREFIX.len())
        .collect();
    // Deterministic order so a parent and child key (`A` and `A__B`) resolve
    // the same way every run.
    vars.sort_by(|a, b| a.0.cmp(b.0));
    for (var, raw) in vars {
        let path: Vec<String> = var[ENV_PREFIX.len()..]
            .split("__")
            .map(|s| s.to_ascii_lowercase())
            .collect();
        if path.iter().any(String::is_empty) {
            return Err(ConfigError::Env {
                var: var.to_owned(),
                message: "empty key segment (check for a stray '__')".into(),
            });
        }
        set_path(table, &path, raw).map_err(|message| ConfigError::Env {
            var: var.to_owned(),
            message,
        })?;
        tracing::debug!(var, key = path.join("."), "applied environment override");
    }
    Ok(())
}

fn set_path(table: &mut Table, path: &[String], raw: &str) -> Result<(), String> {
    let (last, parents) = path.split_last().ok_or_else(|| "empty key".to_owned())?;
    let mut cur = table;
    for seg in parents {
        let entry = cur
            .entry(seg.clone())
            .or_insert_with(|| Value::Table(Table::new()));
        match entry {
            Value::Table(t) => cur = t,
            other => {
                return Err(format!(
                    "cannot descend into '{seg}': it holds a {} not a table",
                    other.type_str()
                ))
            }
        }
    }
    let value = match cur.get(last) {
        // Known key: match its type.
        Some(existing) => coerce_like(existing, raw)?,
        // Module sections have no schema here; guess the type from the text.
        None if path.first().map(String::as_str) == Some("modules") => parse_loose(raw),
        // Anything else in the top-level schema that has no default value
        // (the optional TLS paths) is a string.
        None => Value::String(raw.to_owned()),
    };
    cur.insert(last.clone(), value);
    Ok(())
}

/// Parse `raw` as a TOML value if it is one, otherwise as a string.
fn parse_loose(raw: &str) -> Value {
    let doc = format!("v = {raw}");
    match doc.parse::<Table>() {
        Ok(mut t) => t
            .remove("v")
            .unwrap_or_else(|| Value::String(raw.to_owned())),
        Err(_) => Value::String(raw.to_owned()),
    }
}

/// Parse `raw` to match the type already stored at the key.
fn coerce_like(existing: &Value, raw: &str) -> Result<Value, String> {
    match existing {
        Value::String(_) => Ok(Value::String(raw.to_owned())),
        Value::Integer(_) => raw
            .trim()
            .parse::<i64>()
            .map(Value::Integer)
            .map_err(|_| format!("expected an integer, got {raw:?}")),
        Value::Float(_) => raw
            .trim()
            .parse::<f64>()
            .map(Value::Float)
            .map_err(|_| format!("expected a number, got {raw:?}")),
        Value::Boolean(_) => match raw.trim() {
            "true" | "1" | "yes" | "on" => Ok(Value::Boolean(true)),
            "false" | "0" | "no" | "off" => Ok(Value::Boolean(false)),
            _ => Err(format!("expected a boolean, got {raw:?}")),
        },
        Value::Array(_) => match parse_loose(raw) {
            v @ Value::Array(_) => Ok(v),
            // Convenience: comma-separated words become a string array.
            _ => Ok(Value::Array(
                raw.split(',')
                    .map(|s| Value::String(s.trim().to_owned()))
                    .filter(|v| v.as_str() != Some(""))
                    .collect(),
            )),
        },
        Value::Table(_) => match parse_loose(raw) {
            v @ Value::Table(_) => Ok(v),
            _ => Err(format!("expected an inline table, got {raw:?}")),
        },
        Value::Datetime(_) => match parse_loose(raw) {
            v @ Value::Datetime(_) => Ok(v),
            _ => Err(format!("expected a date-time, got {raw:?}")),
        },
    }
}
