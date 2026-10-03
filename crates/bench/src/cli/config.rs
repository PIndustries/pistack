// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `~/.config/pistack/config.toml`: named profiles and which one is current.
//!
//! ```toml
//! current = "admin"
//!
//! [profiles.admin]
//! url = "https://cloud.example:8443"
//! token = "eyJ..."
//! project = "admin"
//! insecure = false
//! ```

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const DEFAULT_PROFILE: &str = "default";

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Profile {
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub insecure: bool,
    /// Who the token belongs to, for `pi profile show`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub principal: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Config {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current: Option<String>,
    #[serde(default)]
    pub profiles: BTreeMap<String, Profile>,
}

impl Config {
    /// `--config`, else `$PI_CONFIG`, else `~/.config/pistack/config.toml`.
    pub fn default_path() -> PathBuf {
        if let Some(p) = std::env::var_os("PI_CONFIG") {
            return PathBuf::from(p);
        }
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("pistack")
            .join("config.toml")
    }

    /// A missing file is an empty config, not an error.
    pub fn load(path: &Path) -> Result<Config> {
        match std::fs::read_to_string(path) {
            Ok(s) => toml::from_str(&s).with_context(|| format!("parsing {}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
            Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
        }
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        }
        let s = toml::to_string_pretty(self).context("serialising config")?;
        std::fs::write(path, s).with_context(|| format!("writing {}", path.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            // Tokens live here; keep it private.
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
        }
        Ok(())
    }

    pub fn current_name(&self) -> &str {
        self.current.as_deref().unwrap_or(DEFAULT_PROFILE)
    }

    pub fn current(&self) -> Option<&Profile> {
        self.profiles.get(self.current_name())
    }

    pub fn get(&self, name: &str) -> Option<&Profile> {
        self.profiles.get(name)
    }

    pub fn set(&mut self, name: &str, p: Profile) {
        self.profiles.insert(name.to_owned(), p);
        if self.current.is_none() {
            self.current = Some(name.to_owned());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_missing_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let c = Config::load(&dir.path().join("nope.toml")).unwrap();
        assert_eq!(c, Config::default());
        assert_eq!(c.current_name(), DEFAULT_PROFILE);
    }

    #[test]
    fn save_then_load_roundtrips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("config.toml");
        let mut c = Config::default();
        c.set(
            "admin",
            Profile {
                url: "http://127.0.0.1:8443".into(),
                token: Some("tok".into()),
                project: Some("admin".into()),
                insecure: true,
                principal: Some("admin".into()),
                expires_at: None,
            },
        );
        c.set(
            "dev",
            Profile {
                url: "http://localhost:9000".into(),
                ..Default::default()
            },
        );
        c.save(&path).unwrap();
        let back = Config::load(&path).unwrap();
        assert_eq!(back, c);
        // First profile set becomes current.
        assert_eq!(back.current_name(), "admin");
        assert_eq!(back.current().unwrap().token.as_deref(), Some("tok"));
        assert!(!back.get("dev").unwrap().insecure);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("[profiles.admin]"));
        assert!(!text.contains("insecure = false"), "{text}");
    }

    #[test]
    fn parses_hand_written() {
        let c: Config = toml::from_str(
            r#"
current = "lab"
[profiles.lab]
url = "https://lab:8443"
token = "t"
"#,
        )
        .unwrap();
        assert_eq!(c.current().unwrap().url, "https://lab:8443");
        assert!(c.current().unwrap().project.is_none());
    }
}
