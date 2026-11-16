// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use crate::client::functions::*;
use crate::client::Client;
use crate::types::*;
use pistack_store::StoreError;
use pistack_types::{kind, ApiResult, Id, Meta};
impl Client {
    /// Like [`Self::create_port_for_device`]. `groups` replaces the default
    /// security group when it is not empty.
    pub async fn create_port_for_device_with(
        &self,
        project: &Id,
        network: &str,
        device: Id,
        groups: &[Id],
    ) -> ApiResult<Port> {
        let net = match self.networks().resolve(Some(project), network) {
            Ok(n) => n,
            Err(StoreError::NotFound(_)) => {
                self.networks().resolve(None, network).map_err(store_err)?
            }
            Err(e) => return Err(store_err(e)),
        };
        let idx = self.ports_of_device(&device)?.len();
        let mut meta = Meta::new(
            kind::PORT,
            format!("{device}-eth{idx}"),
            Some(project.clone()),
        );
        meta.labels
            .insert("pistack.io/device".into(), device.to_string());
        let mut spec = PortSpec {
            network: net.meta.id.clone(),
            device: Some(device),
            ..PortSpec::default()
        };
        if !groups.is_empty() {
            spec.security_groups = groups.to_vec();
        }
        let port = self.create_port(meta, spec)?;
        self.reconcile_port(&port.meta.id).await;
        self.ports().get(&port.meta.id).map_err(store_err)
    }
}
