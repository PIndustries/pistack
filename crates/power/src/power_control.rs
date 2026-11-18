// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use async_trait::async_trait;
use pistack_types::ApiResult;

use crate::chassis_power::ChassisPower;

/// Power control for one chassis, addressed by its BMC.
#[async_trait]
pub trait PowerControl: Send + Sync {
    /// The transport, `ipmi` or `redfish`, for reports and logs.
    fn kind(&self) -> &'static str;
    /// Power the chassis on.
    async fn power_on(&self) -> ApiResult<()>;
    /// Power the chassis off. Does not confirm the off state; read it back
    /// with [`PowerControl::power_status`].
    async fn power_off(&self) -> ApiResult<()>;
    /// The power state the BMC reports now.
    async fn power_status(&self) -> ApiResult<ChassisPower>;
    /// Point the next boot at the network. Used by provisioning, not by a
    /// fence.
    async fn boot_pxe(&self) -> ApiResult<()>;
}
