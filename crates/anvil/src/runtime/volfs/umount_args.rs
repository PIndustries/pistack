// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

pub(in crate::runtime::volfs) fn umount_args(dir: &str) -> Vec<String> {
    vec![dir.to_owned()]
}
