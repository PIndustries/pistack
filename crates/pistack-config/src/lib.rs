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

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use toml::{Table, Value};

/// Prefix every override variable starts with.
pub const ENV_PREFIX: &str = "PISTACK__";
/// Variable naming the config file when `Config::load(None)` is called.
pub const ENV_CONFIG_PATH: &str = "PISTACK_CONFIG";
/// Paths tried, in order, when no file is given and `PISTACK_CONFIG` is unset.
pub const DEFAULT_PATHS: &[&str] = &["pistack.toml", "/etc/pistack/pistack.toml"];

/// A complete, commented example configuration. It parses to the defaults
/// (see the `example_parses` test), so it doubles as documentation of them.
pub const EXAMPLE_TOML: &str = r#"# pistack.example.toml
#
# Every key is optional; the values shown are the defaults. Any key may be
# overridden from the environment as PISTACK__<SECTION>__<KEY>, e.g.
# PISTACK__SERVER__LISTEN=0.0.0.0:8443 or PISTACK__DATA_DIR=/srv/pistack.

# Where the state store, TLS material and local image cache live.
data_dir = "/var/lib/pistack"

# Name this node reports as. Empty means "use the hostname".
node_name = ""

[server]
# Address the single API port binds to. Every module is a prefix on it.
listen = "0.0.0.0:8443"
# URL clients and agents use to reach this server. Empty means derive it
# from `listen` and the TLS setting.
public_url = ""
# Serve plain HTTP. Only for development; agents will not join over it.
insecure_http = false
# Seconds a long-poll (agent command stream, watch) may hang before the
# server answers with an empty result.
long_poll_secs = 30

[tls]
# PEM files for the API listener. When unset, `keep` issues a certificate
# from the cloud CA into <data_dir>/tls at first start.
# cert = "/etc/pistack/tls/server.crt"
# key = "/etc/pistack/tls/server.key"
# CA bundle used to verify agents and peer servers (mTLS). Defaults to the
# cloud CA in <data_dir>/tls/ca.crt.
# ca = "/etc/pistack/tls/ca.crt"

[log]
# tracing filter: error, warn, info, debug, trace, or a full EnvFilter.
level = "info"
# "text" or "json"
format = "text"

# Module-specific settings. One table per module, by workshop name.
# Each module reads its own with Config::module::<T>("name").
[modules]

# [modules.forge]
# scheduler = "spread"
# [modules.fabric]
# ovn_nb = "tcp:127.0.0.1:6641"
# [modules.slab]
# backends = ["lvm"]
# [modules.silo]
# # Largest single PUT in bytes (default 5 GiB).
# max_object_bytes = 5368709120
# # Seconds before an unfinished multipart upload is swept (default 7 days).
# multipart_ttl_secs = 604800
# [modules.keep]
# # Defaults for projects whose gate quotas do not name `secrets`,
# # `certificates` or `access_keys`; a project quota always wins.
# max_secrets_per_project = 1000
# max_certificates_per_project = 500
# max_access_keys_per_project = 100
# # Cloud-wide: unexpired, unspent join tokens at once, and the most
# # redemptions a multi-use token may allow (also its default).
# max_live_join_tokens = 100
# max_join_token_uses = 100
"#;

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

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct ServerConfig {
    /// `host:port` the API listener binds.
    pub listen: String,
    /// Externally reachable base URL; empty means derive from `listen`.
    pub public_url: String,
    pub insecure_http: bool,
    pub long_poll_secs: u64,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            listen: "0.0.0.0:8443".into(),
            public_url: String::new(),
            insecure_http: false,
            long_poll_secs: 30,
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct TlsConfig {
    pub cert: Option<PathBuf>,
    pub key: Option<PathBuf>,
    pub ca: Option<PathBuf>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct LogConfig {
    pub level: String,
    pub format: LogFormat,
}

impl Default for LogConfig {
    fn default() -> Self {
        Self {
            level: "info".into(),
            format: LogFormat::Text,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum LogFormat {
    #[default]
    Text,
    Json,
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

/// Resolved TLS file locations.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TlsPaths {
    pub cert: PathBuf,
    pub key: PathBuf,
    pub ca: PathBuf,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    #[test]
    fn example_parses_to_defaults() {
        let cfg = Config::from_toml(EXAMPLE_TOML).unwrap();
        assert_eq!(cfg, Config::default());
        assert_eq!(cfg.server.listen, "0.0.0.0:8443");
        assert_eq!(cfg.data_dir, PathBuf::from("/var/lib/pistack"));
        assert!(cfg.modules.is_empty());
    }

    #[test]
    fn empty_document_is_defaults() {
        assert_eq!(Config::from_toml("").unwrap(), Config::default());
    }

    #[test]
    fn toml_roundtrip() {
        let mut cfg = Config::default();
        cfg.server.listen = "127.0.0.1:9000".into();
        cfg.tls.cert = Some("/a.crt".into());
        cfg.tls.key = Some("/a.key".into());
        let mut forge = Table::new();
        forge.insert("scheduler".into(), Value::String("spread".into()));
        cfg.modules.insert("forge".into(), forge);
        let text = cfg.to_toml().unwrap();
        let back = Config::from_toml(&text).unwrap();
        assert_eq!(back, cfg);
    }

    #[test]
    fn file_then_env_overrides() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pistack.toml");
        std::fs::write(
            &path,
            r#"
data_dir = "/srv/pistack"
[server]
listen = "127.0.0.1:8443"
long_poll_secs = 10
[log]
format = "json"
[modules.forge]
scheduler = "pack"
retries = 3
backends = ["lvm"]
"#,
        )
        .unwrap();

        let cfg = Config::load_with_env(
            Some(&path),
            env(&[
                ("PISTACK__SERVER__LISTEN", "0.0.0.0:9443"),
                ("PISTACK__SERVER__INSECURE_HTTP", "true"),
                ("PISTACK__SERVER__LONG_POLL_SECS", "5"),
                ("PISTACK__TLS__CERT", "/etc/pistack/tls.crt"),
                ("PISTACK__TLS__KEY", "/etc/pistack/tls.key"),
                ("PISTACK__MODULES__FORGE__SCHEDULER", "spread"),
                ("PISTACK__MODULES__FORGE__RETRIES", "7"),
                ("PISTACK__MODULES__FORGE__BACKENDS", "lvm,ceph"),
                ("PISTACK__MODULES__FABRIC__OVN_NB", "tcp:10.0.0.1:6641"),
                ("PISTACK__MODULES__FABRIC__MTU", "9000"),
                ("UNRELATED", "x"),
                ("PISTACK_DATA_DIR", "ignored-single-underscore"),
            ]),
        )
        .unwrap();

        assert_eq!(cfg.data_dir, PathBuf::from("/srv/pistack"));
        assert_eq!(cfg.server.listen, "0.0.0.0:9443");
        assert!(cfg.server.insecure_http);
        assert_eq!(cfg.server.long_poll_secs, 5);
        assert_eq!(cfg.log.format, LogFormat::Json);
        assert_eq!(
            cfg.tls.cert.as_deref(),
            Some(Path::new("/etc/pistack/tls.crt"))
        );

        let forge = &cfg.modules["forge"];
        assert_eq!(forge["scheduler"].as_str(), Some("spread"));
        assert_eq!(forge["retries"].as_integer(), Some(7));
        let backends: Vec<&str> = forge["backends"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(Value::as_str)
            .collect();
        assert_eq!(backends, vec!["lvm", "ceph"]);

        // A module not in the file can be created from env alone; values
        // are typed loosely: an integer-looking value becomes an integer,
        // an unparsable one a string.
        let fabric = &cfg.modules["fabric"];
        assert_eq!(fabric["ovn_nb"].as_str(), Some("tcp:10.0.0.1:6641"));
        assert_eq!(fabric["mtu"].as_integer(), Some(9000));
    }

    #[test]
    fn env_keeps_string_type_for_existing_strings() {
        let cfg = Config::load_with_env(
            None,
            env(&[
                ("PISTACK__SERVER__PUBLIC_URL", "true"),
                ("PISTACK__NODE_NAME", "42"),
                ("PISTACK__DATA_DIR", "/tmp/x"),
                ("PISTACK__TLS__CERT", "1"),
                ("PISTACK__TLS__KEY", "2"),
            ]),
        )
        .unwrap();
        assert_eq!(cfg.tls.cert.as_deref(), Some(Path::new("1")));
        // Without a file the table is seeded from the defaults, so these
        // string keys stay strings instead of becoming a bool and an integer.
        assert_eq!(cfg.server.public_url, "true");
        assert_eq!(cfg.node_name, "42");
        assert_eq!(cfg.data_dir, PathBuf::from("/tmp/x"));
    }

    #[test]
    fn env_type_errors_are_reported() {
        let e = Config::load_with_env(None, env(&[("PISTACK__SERVER__INSECURE_HTTP", "maybe")]))
            .unwrap_err();
        assert_eq!(
            e.to_string(),
            "environment variable PISTACK__SERVER__INSECURE_HTTP: expected a boolean, got \"maybe\""
        );
        let e = Config::load_with_env(None, env(&[("PISTACK__SERVER__LONG_POLL_SECS", "soon")]))
            .unwrap_err();
        assert!(e.to_string().contains("expected an integer"), "{e}");
        let e =
            Config::load_with_env(None, env(&[("PISTACK__SERVER____LISTEN", "x")])).unwrap_err();
        assert!(e.to_string().contains("empty key segment"), "{e}");
        let e = Config::load_with_env(None, env(&[("PISTACK__SERVER__LISTEN__PORT", "x")]))
            .unwrap_err();
        assert!(
            e.to_string().contains("cannot descend into 'listen'"),
            "{e}"
        );
    }

    #[test]
    fn unknown_keys_rejected() {
        let e = Config::from_toml("[server]\nlisten = \"x:1\"\nlistne = \"typo\"\n").unwrap_err();
        assert!(matches!(e, ConfigError::Invalid(_)), "{e}");
        assert!(e.to_string().contains("listne"), "{e}");
        let e = Config::load_with_env(None, env(&[("PISTACK__SERVER__NOPE", "1")])).unwrap_err();
        assert!(e.to_string().contains("nope"), "{e}");
    }

    #[test]
    fn validation() {
        let e = Config::from_toml("[server]\nlisten = \"\"\n").unwrap_err();
        assert_eq!(e.to_string(), "config: server.listen must not be empty");
        let e = Config::from_toml("[server]\nlisten = \"nocolon\"\n").unwrap_err();
        assert!(e.to_string().contains("host:port"));
        let e = Config::from_toml("[tls]\ncert = \"/a\"\n").unwrap_err();
        assert_eq!(
            e.to_string(),
            "config: tls.cert and tls.key must be set together"
        );
        let e = Config::from_toml("[server]\nlong_poll_secs = 0\n").unwrap_err();
        assert!(e.to_string().contains("long_poll_secs"));
    }

    #[test]
    fn missing_explicit_file_is_an_error() {
        let e = Config::load_with_env(Some(Path::new("/nonexistent/pistack.toml")), env(&[]))
            .unwrap_err();
        assert!(matches!(e, ConfigError::Read { .. }), "{e}");
        assert!(e
            .to_string()
            .starts_with("config file /nonexistent/pistack.toml:"));
    }

    #[test]
    fn bad_toml_names_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.toml");
        std::fs::write(&path, "this is = = not toml").unwrap();
        let e = Config::load_with_env(Some(&path), env(&[])).unwrap_err();
        assert!(matches!(e, ConfigError::Parse { .. }), "{e}");
        assert!(e.to_string().contains("bad.toml"));
    }

    #[test]
    fn config_path_from_env() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("elsewhere.toml");
        std::fs::write(&path, "node_name = \"n1\"\n").unwrap();
        let cfg =
            Config::load_with_env(None, env(&[(ENV_CONFIG_PATH, path.to_str().unwrap())])).unwrap();
        assert_eq!(cfg.node_name, "n1");
        // Pointing at a missing file is an error, not a silent default.
        let e = Config::load_with_env(None, env(&[(ENV_CONFIG_PATH, "/nope.toml")])).unwrap_err();
        assert!(matches!(e, ConfigError::Read { .. }));
    }

    #[derive(Debug, Default, Deserialize, PartialEq)]
    #[serde(default, deny_unknown_fields)]
    struct ForgeConfig {
        scheduler: String,
        retries: u32,
        backends: Vec<String>,
    }

    #[test]
    fn module_sections() {
        let cfg = Config::from_toml(
            "[modules.forge]\nscheduler = \"spread\"\nretries = 2\nbackends = [\"lvm\"]\n[modules.slab]\nbogus = 1\n",
        )
        .unwrap();
        let forge: ForgeConfig = cfg.module("forge").unwrap();
        assert_eq!(
            forge,
            ForgeConfig {
                scheduler: "spread".into(),
                retries: 2,
                backends: vec!["lvm".into()]
            }
        );
        // Missing section: defaults.
        let fabric: ForgeConfig = cfg.module("fabric").unwrap();
        assert_eq!(fabric, ForgeConfig::default());
        // Present but wrong shape: a module-scoped error.
        let e = cfg.module::<ForgeConfig>("slab").unwrap_err();
        assert!(
            e.to_string().starts_with("module section [modules.slab]:"),
            "{e}"
        );
        assert!(e.to_string().contains("bogus"));
    }

    #[test]
    fn public_url_derivation() {
        let mut cfg = Config::default();
        assert_eq!(cfg.public_url(), "https://localhost:8443");
        cfg.server.insecure_http = true;
        cfg.server.listen = "10.0.0.5:8080".into();
        assert_eq!(cfg.public_url(), "http://10.0.0.5:8080");
        cfg.server.listen = "[::]:8443".into();
        assert_eq!(cfg.public_url(), "http://localhost:8443");
        cfg.server.public_url = "https://cloud.example/".into();
        assert_eq!(cfg.public_url(), "https://cloud.example");
    }

    #[test]
    fn tls_paths_default_under_data_dir() {
        let mut cfg = Config {
            data_dir: "/d".into(),
            ..Config::default()
        };
        let p = cfg.tls_paths();
        assert_eq!(p.cert, PathBuf::from("/d/tls/server.crt"));
        assert_eq!(p.key, PathBuf::from("/d/tls/server.key"));
        assert_eq!(p.ca, PathBuf::from("/d/tls/ca.crt"));
        cfg.tls.ca = Some("/etc/ca.crt".into());
        assert_eq!(cfg.tls_paths().ca, PathBuf::from("/etc/ca.crt"));
    }
}
