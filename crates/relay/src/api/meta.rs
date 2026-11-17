// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The metadata a body carries: a create builds it, an update overwrites
//! the parts the body sent.

use super::body::{Create, Update};
use super::name::check_name;
use pistack_types::{ApiResult, Id, Meta};

pub(super) fn new_meta<S>(k: &'static str, c: &Create<S>, project: Option<Id>) -> ApiResult<Meta> {
    let mut m = Meta::new(k, check_name(&c.name)?, project);
    m.description = c.description.clone();
    m.labels = c.labels.clone();
    m.annotations = c.annotations.clone();
    Ok(m)
}

pub(super) fn apply_meta<S>(m: &mut Meta, u: &Update<S>) {
    if let Some(n) = &u.name {
        m.name = n.trim().to_owned();
    }
    if let Some(d) = &u.description {
        m.description = d.clone();
    }
    if let Some(l) = &u.labels {
        m.labels = l.clone();
    }
    if let Some(a) = &u.annotations {
        m.annotations = a.clone();
    }
}
