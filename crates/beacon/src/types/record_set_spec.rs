// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::Id;
use serde::{Deserialize, Serialize};

use super::record_type::RecordType;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct RecordSetSpec {
    /// Zone the set belongs to.
    pub zone: Id,
    /// Owner name, absolute (`web.example.cloud.`) or relative to the zone
    /// (`web`, or `@` for the apex). Stored absolute and lower-case.
    pub name: String,
    #[serde(rename = "type")]
    pub kind: RecordType,
    /// Falls back to the zone TTL when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttl: Option<u32>,
    /// RDATA in presentation format, one per record:
    /// `10.0.0.4`, `10 mail.example.cloud.`, `"v=spf1 -all"`.
    pub records: Vec<String>,
}
