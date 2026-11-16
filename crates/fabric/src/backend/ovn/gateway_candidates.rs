// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::host_record::HostRecord;
use super::pub_consts::MAX_GATEWAY_CHASSIS;
use pistack_store::Store;
use pistack_types::{ApiError, ApiResult, Phase};

/// Chassis that can host a gateway port, highest priority first: ready
/// hosts whose `ovn-controller` is connected and that have a bridge
/// mapping, ordered by chassis name so every pass gives the same answer.
pub fn gateway_candidates(store: &Store) -> ApiResult<Vec<String>> {
    let hosts = store
        .collection::<HostRecord>()
        .list(None)
        .map_err(|e| ApiError::unavailable(format!("listing hosts: {e}")))?;
    let mut chassis: Vec<String> = hosts
        .into_iter()
        .filter(|h| h.status.phase == Phase::Ready)
        .filter_map(|h| h.status.ovn)
        .filter(|o| o.ready && !o.chassis.is_empty() && !o.bridge_mappings.trim().is_empty())
        .map(|o| o.chassis)
        .collect();
    chassis.sort();
    chassis.dedup();
    chassis.truncate(MAX_GATEWAY_CHASSIS);
    Ok(chassis)
}
