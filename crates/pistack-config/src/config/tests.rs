// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::*;
use serde::Deserialize;
use std::path::{Path, PathBuf};
use toml::{Table, Value};

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
    let e = Config::load_with_env(None, env(&[("PISTACK__SERVER____LISTEN", "x")])).unwrap_err();
    assert!(e.to_string().contains("empty key segment"), "{e}");
    let e =
        Config::load_with_env(None, env(&[("PISTACK__SERVER__LISTEN__PORT", "x")])).unwrap_err();
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
    let e =
        Config::load_with_env(Some(Path::new("/nonexistent/pistack.toml")), env(&[])).unwrap_err();
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
