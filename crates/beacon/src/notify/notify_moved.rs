// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::Client;
use crate::types::ZoneKind;

use super::notified_key::NOTIFIED_KEY;
use super::notify_zones::notify_zones;

/// Every primary zone whose serial has moved since it was last acknowledged,
/// notified to the addresses its secondaries list.
pub(crate) async fn notify_moved(client: &Client) {
    let zones = match client.zones().list(None) {
        Ok(z) => z,
        Err(e) => {
            tracing::warn!(error = %e, "beacon: notify pass");
            return;
        }
    };
    let mut todo = Vec::new();
    for z in zones {
        if !matches!(z.spec.kind, ZoneKind::Primary) || z.status.serial == 0 {
            continue;
        }
        let last = z
            .meta
            .annotations
            .get(NOTIFIED_KEY)
            .and_then(|s| s.parse::<u32>().ok());
        if last == Some(z.status.serial) {
            continue;
        }
        todo.push((z.meta.id.clone(), z.spec.name.clone(), z.status.serial, z));
    }
    notify_zones(client, todo).await;
}
