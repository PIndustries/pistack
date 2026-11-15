// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::overlay::{enabled, shared_nat};
use fabric::{Network, NetworkKind};
use pistack_types::{ApiError, ApiResult};

/// VNI to put on a NIC, or `None` when this NIC stays on host NAT.
///
/// An overlay network with no segment yet is `unavailable`, so the caller
/// can retry after fabric assigns one. Addresses in 10.88.0.0/24 stay on
/// the shared bridge even when the overlay backend is selected.
pub fn segment_for_nic(net: &Network, address: &str) -> ApiResult<Option<u32>> {
    if !enabled()
        || shared_nat(address)
        || !matches!(net.spec.kind, NetworkKind::Overlay)
        || net.status.datapath.as_deref() == Some("hostnat")
    {
        return Ok(None);
    }
    match net.status.segment_id {
        Some(vni) if (1..=16_777_215).contains(&vni) => Ok(Some(vni)),
        _ => Err(ApiError::unavailable(format!(
            "overlay network {} has no segment",
            net.meta.id
        ))),
    }
}
