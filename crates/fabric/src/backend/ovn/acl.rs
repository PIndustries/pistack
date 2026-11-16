// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

/// One ACL as fabric wants it. `rule` only feeds `external_ids`.
#[derive(Clone, Debug)]
pub(in crate::backend::ovn) struct Acl {
    pub(in crate::backend::ovn) direction: &'static str,
    pub(in crate::backend::ovn) priority: i64,
    pub(in crate::backend::ovn) matches: String,
    pub(in crate::backend::ovn) action: &'static str,
    pub(in crate::backend::ovn) rule: Option<String>,
}

impl Acl {
    pub(in crate::backend::ovn) fn key(&self) -> (String, i64, String, String) {
        (
            self.direction.to_string(),
            self.priority,
            self.matches.clone(),
            self.action.to_string(),
        )
    }
}
