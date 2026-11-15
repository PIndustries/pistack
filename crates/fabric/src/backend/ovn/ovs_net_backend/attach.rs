// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::super::attached::Attached;
use super::super::ovn_backend::OvnBackend;
use pistack_store::Store;
use pistack_types::Leadership;

impl OvnBackend {
    pub(in crate::backend::ovn::ovs_net_backend) fn attach_body(
        &self,
        leadership: &Leadership,
        store: &Store,
    ) {
        let _ = self.attached.set(Attached {
            leadership: leadership.clone(),
            store: store.clone(),
        });
    }
}
