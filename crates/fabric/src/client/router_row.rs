// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use crate::client::functions::*;
use crate::client::Client;
use crate::types::*;
use pistack_types::{kind, ApiResult, Id, Meta};
impl Client {
    /// Store one router row. The only place that does, so a router a VPC owns
    /// can be made without the project's `routers` limit being spent on it.
    pub(crate) fn router_row(&self, mut meta: Meta, spec: RouterSpec) -> ApiResult<Router> {
        if meta.id.kind != kind::ROUTER || meta.id.uuid.is_nil() {
            meta.id = Id::new(kind::ROUTER);
        }
        self.routers()
            .create(Router {
                meta,
                spec,
                status: RouterStatus::default(),
            })
            .map_err(store_err)
    }
}
