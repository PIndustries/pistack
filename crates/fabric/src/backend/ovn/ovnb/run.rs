// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::super::{consts::DB, ovn_backend::OvnBackend};
use pistack_types::{ApiError, ApiResult};
use serde_json::Value;

impl OvnBackend {
    /// One transaction, only while leader, abandoned if leadership drops.
    pub(in crate::backend::ovn) async fn run(&self, ops: Vec<Value>) -> ApiResult<Vec<Value>> {
        let Some(a) = self.attached.get() else {
            return self.nb.transact(DB, ops).await;
        };
        if !a.leadership.is_leader() {
            return Err(ApiError::unavailable("not leader; skipping write"));
        }
        let mut rx = a.leadership.subscribe();
        let lost = async move {
            loop {
                if rx.changed().await.is_err() || !*rx.borrow() {
                    return;
                }
            }
        };
        tokio::select! {
            r = self.nb.transact(DB, ops) => r,
            _ = lost => Err(ApiError::unavailable("lost leadership; write abandoned")),
        }
    }
}
