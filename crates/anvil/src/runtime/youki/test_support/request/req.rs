// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::MachineKind;
use crate::runtime::MachineRequest;
use pistack_types::kind;
use pistack_types::Id;
use pistack_types::Resources;

pub(crate) fn req(name: &str, image: &str, address: &str) -> MachineRequest {
    MachineRequest {
        id: Id::new(kind::MACHINE),
        name: name.into(),
        kind: MachineKind::Container,
        image: image.into(),
        image_source: None,
        resources: Resources {
            vcpus: 1,
            memory_mib: 128,
            disk_gib: 1,
            gpus: 0,
        },
        networks: vec![],
        nics: vec![crate::runtime::NicAttachment {
            address: address.into(),
            ..Default::default()
        }],
        volumes: vec![],
        user_data: Some("do-not-log-user-data".into()),
        ssh_keys: vec!["do-not-log-ssh-key".into()],
        overlay: None,
        bmc_address: None,
        bmc_user: None,
    }
}
