// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::Client;
use crate::types::ZoneKind;

use super::notify_zones::notify_zones;
use super::peers_for::peers_for;

/// Notify one named zone now, whether or not its serial moved. Returns the
/// number of peers asked.
pub async fn notify_one(client: &Client, apex: &str) -> usize {
    let Ok(Some(z)) = client.zone_by_apex(apex) else {
        return 0;
    };
    if !matches!(z.spec.kind, ZoneKind::Primary) || z.status.serial == 0 {
        return 0;
    }
    let n = peers_for(client, &z).map(|p| p.len()).unwrap_or(0);
    notify_zones(
        client,
        vec![(z.meta.id.clone(), z.spec.name.clone(), z.status.serial, z)],
    )
    .await;
    n
}
