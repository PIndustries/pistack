// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::Host;
use async_trait::async_trait;
use pistack_store::Store;
use pistack_types::{ApiError, ApiResult};
use std::time::Duration;

use super::fence_poll::FENCE_POLL;
use super::fence_timeout::FENCE_TIMEOUT;
use super::fencer::Fencer;

/// The real fencer: drives a host's BMC over IPMI or Redfish with the
/// shared `power` crate, and reads the BMC password from the secret named
/// on the host. The password is read only here, only for one power-off,
/// and is never returned or logged.
pub struct BmcFencer {
    pub(crate) store: Store,
    pub(crate) timeout: Duration,
    pub(crate) poll: Duration,
}

impl BmcFencer {
    pub fn new(store: Store) -> Self {
        Self {
            store,
            timeout: FENCE_TIMEOUT,
            poll: FENCE_POLL,
        }
    }

    /// The BMC password for `host`, read from the secret its power control
    /// names. A host with no power control, or naming a secret that is not
    /// there, cannot be fenced.
    pub(crate) fn password(&self, pc: &crate::types::PowerControl) -> ApiResult<Vec<u8>> {
        let client = keep::Client::open(&self.store)
            .map_err(|_| ApiError::unavailable("the secrets service is unavailable"))?;
        let listed = client
            .list_secrets(None)
            .map_err(|_| ApiError::unavailable("the secrets service is unavailable"))?;
        let Some(secret) = listed
            .into_iter()
            .find(|s| s.meta.name == pc.secret || s.meta.id.to_string() == pc.secret)
        else {
            return Err(ApiError::unavailable(format!(
                "host power control names a secret, {:?}, that is not there",
                pc.secret
            )));
        };
        client
            .get_secret_payload(&secret.meta.id)
            .map_err(|_| ApiError::unavailable("the host's BMC password could not be read"))
    }
}

#[async_trait]
impl Fencer for BmcFencer {
    async fn fence(&self, host: &Host) -> ApiResult<()> {
        let pc = host
            .spec
            .power_control
            .as_ref()
            .ok_or_else(|| ApiError::conflict("host has no power control"))?;
        let kind = power::Kind::parse(&pc.kind)?;
        let secret = power::Secret::Bytes(self.password(pc)?);
        let control = kind.control(pc.address.clone(), pc.user.clone(), secret);
        control.power_off().await?;
        let deadline = tokio::time::Instant::now() + self.timeout;
        loop {
            match control.power_status().await {
                Ok(power::ChassisPower::Off) => return Ok(()),
                Ok(_) | Err(_) if tokio::time::Instant::now() >= deadline => {
                    return Err(ApiError::unavailable(format!(
                        "host {} was told to power off but is not confirmed off within {}s",
                        host.meta.name,
                        self.timeout.as_secs()
                    )));
                }
                Ok(_) | Err(_) => tokio::time::sleep(self.poll).await,
            }
        }
    }
}
