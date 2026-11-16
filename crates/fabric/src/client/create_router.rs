// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use crate::client::functions::*;
use crate::client::Client;
use crate::types::*;
use crate::usage::{self, admitted};
use pistack_types::{kind, ApiError, ApiResult, Id, Meta};
impl Client {
    pub fn create_router(&self, mut meta: Meta, spec: RouterSpec) -> ApiResult<Router> {
        if spec.vpc.is_some() {
            return Err(ApiError::bad_request(
                "a vpc makes its own router; it is shown as owned and not made here",
            ));
        }
        meta.id = Id::new(kind::ROUTER);
        validate_routes(&spec.routes)?;
        let ext = match &spec.external_network {
            Some(n) => Some(self.external_network(n)?),
            None => None,
        };
        let interfaces = spec.interfaces.clone();
        let spec = RouterSpec {
            interfaces: Vec::new(),
            ..spec
        };
        let project = meta.project.clone();
        // Only the row is stored under the lock. The ports below are made
        // through create_port, which takes it for a counted port.
        let router = admitted(&self.store, project.as_ref(), usage::ROUTERS, || {
            self.router_row(meta, spec)
        })?;
        let plumb = (|| -> ApiResult<Router> {
            if let Some(net) = &ext {
                self.create_gateway_port(&router, net)?;
            }
            if interfaces.is_empty() {
                Ok(router.clone())
            } else {
                self.set_router_interfaces(&router.meta.id, None, &interfaces)
            }
        })();
        match plumb {
            Ok(r) => Ok(r),
            Err(e) => {
                // Roll back so a failed create leaves nothing behind.
                let _ = self.delete_router(&router.meta.id);
                Err(e)
            }
        }
    }
}
