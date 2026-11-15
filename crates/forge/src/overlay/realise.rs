// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use fabric::overlay::{plan, PlanHost, PlanNetwork, SegmentPlan};
use pistack_types::{ApiError, ApiResult};
use std::net::Ipv4Addr;

pub(in crate::overlay) fn realise(
    underlay: Ipv4Addr,
    network: PlanNetwork,
    force_metadata: bool,
) -> ApiResult<SegmentPlan> {
    let planned = plan(
        &PlanHost {
            underlay: underlay.to_string(),
            underlay_mtu: 0,
        },
        &[network],
        &[],
    )
    .map_err(|e| ApiError::unprocessable(e.to_string()))?;
    let mut segment = planned
        .networks
        .into_iter()
        .next()
        .ok_or_else(|| ApiError::internal("overlay plan had no network"))?;
    // Metadata is answered on the bridge of every host that has a tap,
    // including hosts that do not hold the router namespace.
    if force_metadata {
        segment.metadata = true;
    }
    Ok(segment)
}
