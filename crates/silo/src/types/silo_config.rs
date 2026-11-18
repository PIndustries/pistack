// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use serde::{Deserialize, Serialize};

use super::default_region::DEFAULT_REGION;
use super::max_object_bytes::MAX_OBJECT_BYTES;
use super::multipart_ttl_secs::MULTIPART_TTL_SECS;

/// Module settings under `[modules.silo]` / `PISTACK__MODULES__SILO__*`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct SiloConfig {
    /// Largest accepted object body, in bytes.
    pub max_object_bytes: usize,
    /// Seconds after initiation before an unfinished multipart upload's
    /// staging is swept (default [`MULTIPART_TTL_SECS`]).
    pub multipart_ttl_secs: u64,
    /// Region name accepted in SigV4 credentials (default [`DEFAULT_REGION`]).
    pub region: String,
    /// Seconds between pulls of object bytes this server lacks from the
    /// other control-plane servers (default
    /// [`crate::replica::REPLICATE_INTERVAL_SECS`]). `0` turns the pull
    /// off; a read still fetches what it needs.
    pub replicate_interval_secs: u64,
    /// Servers that must hold an object's bytes before a put is answered
    /// (default [`crate::replica::PUT_COPIES`]). `1` answers as soon as
    /// this server has them. A cluster of one server never waits.
    pub put_copies: u32,
}

impl Default for SiloConfig {
    fn default() -> Self {
        Self {
            max_object_bytes: MAX_OBJECT_BYTES,
            multipart_ttl_secs: MULTIPART_TTL_SECS,
            region: DEFAULT_REGION.to_owned(),
            replicate_interval_secs: crate::replica::REPLICATE_INTERVAL_SECS,
            put_copies: crate::replica::PUT_COPIES,
        }
    }
}

impl SiloConfig {
    /// Reject `max_object_bytes = 0`, `multipart_ttl_secs = 0`, or an empty
    /// `region` (unset fields use their defaults).
    pub fn validate(&self) -> Result<(), pistack_config::ConfigError> {
        let invalid = |message: &str| pistack_config::ConfigError::Module {
            module: "silo".into(),
            message: message.into(),
        };
        if self.max_object_bytes == 0 {
            return Err(invalid("max_object_bytes must be greater than 0"));
        }
        if self.multipart_ttl_secs == 0 {
            return Err(invalid("multipart_ttl_secs must be greater than 0"));
        }
        if self.region.trim().is_empty() {
            return Err(invalid("region must not be empty"));
        }
        if self.put_copies == 0 {
            return Err(invalid("put_copies must be 1 or more"));
        }
        Ok(())
    }

    /// How long an unfinished multipart upload may sit before the sweep
    /// removes it.
    pub fn multipart_ttl(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.multipart_ttl_secs)
    }

    /// Read `[modules.silo]` from a loaded config. Missing section → defaults.
    /// A present section with `max_object_bytes = 0`,
    /// `multipart_ttl_secs = 0`, or an empty `region` is rejected.
    pub fn from_config(cfg: &pistack_config::Config) -> Result<Self, pistack_config::ConfigError> {
        let silo: Self = cfg.module("silo")?;
        silo.validate()?;
        Ok(silo)
    }

    /// Load process config (file + `PISTACK__*`) and take `[modules.silo]`.
    ///
    /// No config file → defaults (5 GiB, 7-day multipart TTL, region
    /// `pistack`). Invalid silo settings (including a zero size or TTL, or
    /// empty region) return an error so assemble can refuse to start.
    pub fn load() -> Result<Self, pistack_config::ConfigError> {
        let cfg = pistack_config::Config::load(None)?;
        Self::from_config(&cfg)
    }
}
