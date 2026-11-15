// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::super::{group_acls::group_acls, group_name::group_name, ovn_backend::OvnBackend};
use crate::types::{Remote, SecurityGroup};
use pistack_types::ApiResult;

impl OvnBackend {
    /// The group for `sg` with its rules as ACLs. Groups its rules refer to
    /// are created empty first, so the address sets OVN derives from them
    /// exist before an ACL names them.
    pub(in crate::backend::ovn) async fn ensure_sg_group(
        &self,
        sg: &SecurityGroup,
        ports: Option<Vec<String>>,
    ) -> ApiResult<()> {
        for rule in &sg.spec.rules {
            if let Remote::Group(g) = &rule.remote {
                if g != &sg.meta.id {
                    self.ensure_group(&group_name(g), self.tags(g, None), None, None)
                        .await?;
                }
            }
        }
        let acls = group_acls(sg);
        self.ensure_group(
            &group_name(&sg.meta.id),
            self.tags(&sg.meta.id, sg.meta.project.as_ref()),
            Some(&acls),
            ports,
        )
        .await
    }
}
