// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use serde::{Deserialize, Serialize};

/// Primary zones are edited here and served by beacon (including AXFR over
/// TCP). Secondary zones list `primaries` and are filled by AXFR pull: they
/// stay Pending until a transfer succeeds, then Ready and served from the
/// secondary cache (record sets are still primary-only).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ZoneKind {
    #[default]
    Primary,
    Secondary {
        /// Addresses (`ip` or `ip:port`) of the primaries to transfer from.
        primaries: Vec<String>,
    },
}
