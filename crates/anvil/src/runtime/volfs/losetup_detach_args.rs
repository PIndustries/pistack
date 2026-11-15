// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

pub(in crate::runtime::volfs) fn losetup_detach_args(device: &str) -> Vec<String> {
    vec!["-d".to_owned(), device.to_owned()]
}
