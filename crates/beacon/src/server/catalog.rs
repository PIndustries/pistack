// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::Client;
use crate::types::in_zone;
use hickory_proto::rr::Name;

use super::compile_zone::compile_zone;
use super::zone_data::ZoneData;

/// Every primary zone, compiled.
#[derive(Default)]
pub struct Catalog {
    pub(crate) zones: Vec<ZoneData>,
}

impl Catalog {
    pub(crate) fn build(client: &Client) -> anyhow::Result<Self> {
        let mut zones = Vec::new();
        for z in client.zones().list(None)? {
            if z.spec.kind != crate::types::ZoneKind::Primary {
                continue;
            }
            match compile_zone(client, &z) {
                Ok(d) => zones.push(d),
                Err(e) => tracing::warn!(zone = %z.spec.name, error = %e, "beacon: zone skipped"),
            }
        }
        Ok(Self { zones })
    }

    /// The zone with the longest apex containing `name`.
    pub(crate) fn find(&self, name: &Name) -> Option<&ZoneData> {
        let n = name.to_ascii();
        self.zones
            .iter()
            .filter(|z| in_zone(&z.apex.to_ascii(), &n))
            .max_by_key(|z| z.apex.num_labels())
    }

    /// Exact apex match, for AXFR (the query name must be the zone name).
    pub(crate) fn find_apex(&self, name: &Name) -> Option<&ZoneData> {
        self.zones.iter().find(|z| &z.apex == name)
    }
}
