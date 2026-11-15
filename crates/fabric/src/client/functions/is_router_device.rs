// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use pistack_types::{kind, Id};
pub(crate) fn is_router_device(d: &Option<Id>) -> bool {
    d.as_ref().is_some_and(|d| d.kind == kind::ROUTER)
}
