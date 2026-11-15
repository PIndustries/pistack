// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::volumes::mount_key;
use crate::runtime::youki::volumes::mountinfo_point;

pub(in crate::runtime::youki) fn mountinfo_lists(text: &str, path: &str) -> bool {
    let want = mount_key(path);
    text.lines()
        .any(|line| mountinfo_point(line).is_some_and(|point| mount_key(&point) == want))
}
