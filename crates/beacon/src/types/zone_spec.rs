// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use serde::{Deserialize, Serialize};

use super::default_email::default_email;
use super::default_ttl::default_ttl;
use super::zone_kind::ZoneKind;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ZoneSpec {
    /// Apex name, absolute with a trailing dot: `example.cloud.`.
    pub name: String,
    #[serde(default)]
    pub kind: ZoneKind,
    /// Default TTL for records that do not set one, and the SOA minimum.
    #[serde(default = "default_ttl")]
    pub ttl: u32,
    /// Hostmaster address, written into the SOA RNAME in mailbox form.
    #[serde(default = "default_email")]
    pub email: String,
    /// Authoritative name servers, absolute names. The first is the SOA
    /// MNAME. Defaults to `ns1.<zone>` when empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub nameservers: Vec<String>,
    /// Addresses (`ip` or `ip:port`) of servers outside this cloud that are
    /// secondaries of a primary zone, and are therefore told when its serial
    /// moves. A secondary zone does not take this field.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub also_notify: Vec<String>,
}

impl Default for ZoneSpec {
    fn default() -> Self {
        Self {
            name: String::new(),
            kind: ZoneKind::Primary,
            ttl: default_ttl(),
            email: default_email(),
            nameservers: Vec::new(),
            also_notify: Vec::new(),
        }
    }
}
