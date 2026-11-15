// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

pub(in crate::runtime::volfs) fn blkid_args(device: &str) -> Vec<String> {
    vec![
        "-p".to_owned(),
        "-o".to_owned(),
        "value".to_owned(),
        "-s".to_owned(),
        "TYPE".to_owned(),
        device.to_owned(),
    ]
}
