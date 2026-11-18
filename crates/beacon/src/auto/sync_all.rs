// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::Client;
use pistack_types::ApiResult;

use super::auto_zones::auto_zones;
use super::machine_addresses::machine_addresses;
use super::sync_zone::sync_zone;

/// One full pass over every auto zone. Returns the number of sets written
/// or removed.
pub fn sync_all(client: &Client) -> ApiResult<usize> {
    let zones = auto_zones(client)?;
    if zones.is_empty() {
        return Ok(0);
    }
    let machines = machine_addresses(client)?;
    let mut changed = 0;
    for zone in zones {
        changed += sync_zone(client, &zone, &machines)?;
    }
    Ok(changed)
}
