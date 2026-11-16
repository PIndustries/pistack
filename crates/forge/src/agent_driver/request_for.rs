// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::kind_to_wire::kind_to_wire;
use super::resolve_boot_image::resolve_boot_image;
use super::rpc_driver::RpcDriver;
use crate::types::Machine;
use pistack_rpc::proto::MachineRequest;
impl RpcDriver {
    /// Build the wire request for `m`.
    ///
    /// A bare image name is replaced when the store has a ready `img` of
    /// that name the machine's project may use, whose `file_path` or `backing_ref` is an absolute file
    /// that exists. Anything else, including `ubuntu-24.04` and a
    /// `sha256:` blob id, is sent unchanged so anvil can still resolve
    /// `bases/<name>.qcow2`. A catalog image with a digest also sets
    /// `image_source`, so a host without the file can download it.
    pub fn request_for(&self, m: &Machine) -> MachineRequest {
        let (image, image_source) =
            resolve_boot_image(self.store.as_ref(), m.meta.project.as_ref(), &m.spec.image);
        MachineRequest {
            id: m.meta.id.clone(),
            name: m.meta.name.clone(),
            kind: kind_to_wire(m.spec.kind),
            image,
            image_source,
            resources: self.resources_for(m),
            networks: m.spec.networks.clone(),
            nics: self
                .store
                .as_ref()
                .and_then(|store| crate::nic::attachments(store, m).ok())
                .unwrap_or_default(),
            volumes: m.spec.volumes.clone(),
            user_data: m.spec.user_data.clone(),
            ssh_keys: m.spec.ssh_keys.clone(),
            overlay: None,
            bmc_address: m.spec.bmc_address.clone(),
            bmc_user: m.spec.bmc_user.clone(),
        }
    }
}
