// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use crate::client::functions::*;
use crate::client::Client;
use crate::types::*;
use crate::usage::{self, released};
use pistack_types::{ApiError, ApiResult, Id};
impl Client {
    pub fn delete_router(&self, id: &Id) -> ApiResult<Router> {
        let router = self.routers().get(id).map_err(store_err)?;
        if router.spec.vpc.is_some() {
            return Err(ApiError::conflict(format!(
                "router {} belongs to vpc {}; it goes when the vpc does",
                router.meta.id,
                self.vpc_of_router(&router)
            )));
        }
        for f in self.floating_ips().list(None).map_err(store_err)? {
            if f.status.router.as_ref() == Some(id) && f.spec.port.is_some() {
                self.floating_ips()
                    .modify(&f.meta.id, |x| {
                        x.spec.port = None;
                        Ok(())
                    })
                    .map_err(store_err)?;
            }
        }
        for p in self.ports_of_device(id)? {
            self.delete_port_unchecked(&p)?;
        }
        let _ = router;
        let router = self.routers().delete(id).map_err(store_err)?;
        released(&self.store, router.meta.project.as_ref(), usage::ROUTERS);
        Ok(router)
    }
}
