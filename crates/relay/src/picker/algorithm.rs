// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The balancing algorithms as a value on a pool spec: the wire names, the
//! names a spec may carry, and the refusal of anything else.

use super::select::Algo;
use crate::types::Algorithm;

impl std::fmt::Display for Algorithm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

impl Algorithm {
    /// The wire name.
    pub fn name(self) -> &'static str {
        match self {
            Self::RoundRobin => "round_robin",
            Self::LeastConn => "least_conn",
            Self::SourceIp => "source_ip",
        }
    }

    /// Every name a pool spec may carry.
    pub const NAMES: [&'static str; 3] = ["round_robin", "least_conn", "source_ip"];

    /// Refused at create and update when a pool spec carries anything else.
    pub fn parse(value: &str) -> Result<Self, String> {
        match value.trim() {
            "round_robin" => Ok(Self::RoundRobin),
            "least_conn" => Ok(Self::LeastConn),
            "source_ip" => Ok(Self::SourceIp),
            other => Err(format!(
                "algorithm {other:?} is not a balancing algorithm; use one of {}",
                Self::NAMES.join(", ")
            )),
        }
    }

    pub(super) fn code(self) -> u8 {
        match self {
            Self::RoundRobin => 0,
            Self::LeastConn => 1,
            Self::SourceIp => 2,
        }
    }

    pub(super) fn from_code(code: u8) -> Self {
        match code {
            1 => Self::LeastConn,
            2 => Self::SourceIp,
            _ => Self::RoundRobin,
        }
    }

    pub(super) fn algo(self) -> Algo {
        match self {
            Self::RoundRobin => Algo::RoundRobin,
            Self::LeastConn => Algo::LeastConn,
            Self::SourceIp => Algo::SourceIp,
        }
    }
}

/// A [`serde`] module so a pool spec carrying an unknown algorithm is
/// refused where it is read, with the names it may hold instead.
pub mod wire {
    use super::Algorithm;
    use serde::de::{Deserialize, Deserializer};

    pub fn deserialize<'de, D>(d: D) -> Result<Algorithm, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = String::deserialize(d)?;
        Algorithm::parse(&raw).map_err(serde::de::Error::custom)
    }
}
