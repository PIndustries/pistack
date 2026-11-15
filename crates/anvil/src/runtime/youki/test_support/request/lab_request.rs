// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::super::sandbox_doc;
use super::req;
use crate::runtime::MachineRequest;
use pistack_types::kind;
use pistack_types::Id;

pub(crate) fn lab_request(image: &str) -> MachineRequest {
    let mut request = req("web", "not-a-path", "10.90.0.10");
    request.id = Id::parse(kind::MACHINE, "mach-018f3a2b-1c4d-7e5f-8a6b-0c1d2e3f4a5b").unwrap();
    request.networks = vec!["labnet".into()];
    request.nics = vec![
        crate::runtime::NicAttachment {
            network_id: "net-01a102de-3f33-73bf-86f7-57fe3b47f1ea".into(),
            mac: "52:54:00:AA:BB:CC".into(),
            address: "10.90.0.10".into(),
            prefix: 24,
            gateway: "10.90.0.1".into(),
            routed: true,
            segment_id: None,
            binding: None,
        },
        crate::runtime::NicAttachment {
            network_id: "net-01a102de-3f33-73bf-86f7-57fe3b47f1ea".into(),
            mac: "52:54:00:11:22:33".into(),
            address: "10.90.0.11".into(),
            prefix: 24,
            gateway: String::new(),
            routed: false,
            segment_id: Some(4),
            binding: None,
        },
    ];
    request.user_data = Some(sandbox_doc(image));
    request
}
