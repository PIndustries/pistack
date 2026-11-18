// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::{kind, Kind, Meta, Resource};
use serde::{Deserialize, Serialize};

use super::zone_spec::ZoneSpec;
use super::zone_status::ZoneStatus;

resource!(
    /// An authoritative DNS zone.
    Zone,
    ZoneSpec,
    ZoneStatus,
    kind::ZONE
);

impl Zone {
    /// SOA MNAME and NS owner names: the spec's list, or `ns1.<zone>`.
    pub fn nameservers(&self) -> Vec<String> {
        if self.spec.nameservers.is_empty() {
            vec![format!("ns1.{}", self.spec.name)]
        } else {
            self.spec.nameservers.clone()
        }
    }

    /// SOA RNAME: `hostmaster@example.cloud` becomes `hostmaster.example.cloud.`;
    /// a bare local part is placed in the zone itself.
    pub fn soa_rname(&self) -> String {
        let (local, domain) = match self.spec.email.split_once('@') {
            Some((l, d)) => (l.to_owned(), format!("{}.", d.trim_end_matches('.'))),
            None => (self.spec.email.clone(), self.spec.name.clone()),
        };
        format!("{}.{}", local.replace('.', "\\."), domain)
    }
}
