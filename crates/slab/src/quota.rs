// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Extra quota hook. Project `volumes` limits are enforced through `gate`
//! in `Client`. This trait is an additional check; `AllowAll` is the default.

use async_trait::async_trait;
use pistack_types::{ApiResult, Id};

/// What a request would consume, in slab's own units.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuotaRequest {
    /// `volumes`, `gigabytes`, `snapshots`, `backups`.
    pub resource: &'static str,
    pub amount: u64,
}

impl QuotaRequest {
    pub fn volumes(n: u64) -> Self {
        Self {
            resource: "volumes",
            amount: n,
        }
    }
    pub fn gigabytes(n: u64) -> Self {
        Self {
            resource: "gigabytes",
            amount: n,
        }
    }
    pub fn snapshots(n: u64) -> Self {
        Self {
            resource: "snapshots",
            amount: n,
        }
    }
    pub fn backups(n: u64) -> Self {
        Self {
            resource: "backups",
            amount: n,
        }
    }
}

#[async_trait]
pub trait QuotaCheck: Send + Sync + 'static {
    /// Return `Err(ApiError::quota(..))` to refuse. `project` is `None` for
    /// cloud-scoped requests, which are never quota-limited.
    async fn check(&self, project: Option<&Id>, requests: &[QuotaRequest]) -> ApiResult<()>;
}

/// Default: everything is allowed.
#[derive(Default, Clone, Copy, Debug)]
pub struct AllowAll;

#[async_trait]
impl QuotaCheck for AllowAll {
    async fn check(&self, _project: Option<&Id>, _requests: &[QuotaRequest]) -> ApiResult<()> {
        Ok(())
    }
}
