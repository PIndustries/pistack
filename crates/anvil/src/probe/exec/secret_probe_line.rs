// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

pub(in crate::probe) fn secret_probe_line(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    const MARKERS: &[&str] = &[
        "begin private key",
        "begin openssh",
        "begin rsa private key",
        "begin ec private key",
        "ssh-ed25519",
        "ssh-rsa",
        "authorized_keys",
    ];
    MARKERS.iter().any(|marker| lower.contains(marker))
}
