// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::applied::Applied;

pub(crate) fn tag(out: &mut Applied, fresh: bool, id: String) {
    if fresh {
        out.created.push(id);
    } else {
        out.present.push(id);
    }
}
