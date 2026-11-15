// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::{acl::Acl, acl_row_key::acl_row_key};
use serde_json::Value;

pub(in crate::backend::ovn) fn same_acls(have: &[Value], want: &[Acl]) -> bool {
    let mut h: Vec<_> = have.iter().map(acl_row_key).collect();
    let mut w: Vec<_> = want.iter().map(Acl::key).collect();
    h.sort();
    w.sort();
    h == w
}
