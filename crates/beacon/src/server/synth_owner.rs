// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use hickory_proto::rr::{Name, Record};

/// A wildcard match is returned under the queried name, not `*.zone`.
pub(crate) fn synth_owner(mut r: Record, owner: &Name) -> Record {
    if r.name.is_wildcard() {
        r.name = owner.clone();
    }
    r
}
