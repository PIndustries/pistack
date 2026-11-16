// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::ApiError;

pub(in crate::runtime::youki) fn container_missing(err: &ApiError) -> bool {
    let m = err.message.to_ascii_lowercase();
    m.contains("not found")
        || m.contains("does not exist")
        || m.contains("doesn't exist")
        || m.contains("no such container")
}
