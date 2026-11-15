// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use crate::client::functions::*;
use crate::client::Client;
use crate::types::*;
use crate::usage::{self, released};
use pistack_types::ApiResult;
impl Client {
    pub(crate) fn delete_port_unchecked(&self, port: &Port) -> ApiResult<Port> {
        for f in self.floating_ips().list(None).map_err(store_err)? {
            if f.spec.port.as_ref() == Some(&port.meta.id) {
                self.floating_ips()
                    .modify(&f.meta.id, |x| {
                        x.spec.port = None;
                        Ok(())
                    })
                    .map_err(store_err)?;
            }
        }
        let counted = !port.is_router_port();
        let port = self.ports().delete(&port.meta.id).map_err(store_err)?;
        if counted {
            released(&self.store, port.meta.project.as_ref(), usage::PORTS);
        }
        Ok(port)
    }
}
