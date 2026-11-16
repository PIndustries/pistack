// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

pub(in crate::runtime::volfs) fn mount_args(
    device: &str,
    dir: &str,
    read_only: bool,
) -> Vec<String> {
    let mut args = Vec::new();
    if read_only {
        args.push("-o".to_owned());
        args.push("ro".to_owned());
    }
    args.push(device.to_owned());
    args.push(dir.to_owned());
    args
}
