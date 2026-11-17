// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! How a pool spreads new connections. Both data planes run these through
//! [`crate::picker`], so a pool balances the same over TCP and over HTTP.

use serde::{Deserialize, Serialize};

/// How a pool spreads new connections. Both data planes run these through
/// [`crate::picker`], so a pool balances the same over TCP and over HTTP.
/// A value outside the three below is refused when the pool is read.
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq, Hash, Default)]
#[serde(rename_all = "snake_case")]
pub enum Algorithm {
    /// Each member is visited a number of times in proportion to its
    /// weight. The default.
    #[default]
    #[serde(deserialize_with = "crate::picker::wire::deserialize")]
    RoundRobin,
    /// The healthy member with the fewest open connections. A tie goes to
    /// the heavier member, then to the earlier one.
    #[serde(deserialize_with = "crate::picker::wire::deserialize")]
    LeastConn,
    /// One client address stays on one healthy member while the member set
    /// is unchanged.
    #[serde(deserialize_with = "crate::picker::wire::deserialize")]
    SourceIp,
}

impl<'de> Deserialize<'de> for Algorithm {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        crate::picker::wire::deserialize(deserializer)
    }
}
