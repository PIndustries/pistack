// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::{acl::Acl, group_name::group_name, rule_acl::rule_acl};
use crate::types::SecurityGroup;

pub(in crate::backend::ovn) fn group_acls(sg: &SecurityGroup) -> Vec<Acl> {
    let name = group_name(&sg.meta.id);
    let mut out: Vec<Acl> = Vec::new();
    for a in sg.spec.rules.iter().filter_map(|r| rule_acl(&name, r)) {
        if !out.iter().any(|o| o.key() == a.key()) {
            out.push(a);
        }
    }
    out
}
