// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::Host;
use async_trait::async_trait;
use pistack_types::ApiResult;

/// Cuts a host's power from outside and proves it is off. Reached through
/// a trait so an evacuation can be tested against a fake; the real one
/// drives the host's BMC over IPMI or Redfish.
#[async_trait]
pub trait Fencer: Send + Sync {
    /// Power `host` off and confirm the off state. Errors if the host has
    /// no power control, or is not confirmed off within [`FENCE_TIMEOUT`].
    async fn fence(&self, host: &Host) -> ApiResult<()>;
}
