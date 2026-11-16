// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

pub(in crate::runtime::volfs) fn losetup_find_args(file: &str) -> Vec<String> {
    vec!["--find".to_owned(), "--show".to_owned(), file.to_owned()]
}
