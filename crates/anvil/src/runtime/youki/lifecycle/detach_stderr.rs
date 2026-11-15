// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::lifecycle::DetachIo;

pub(in crate::runtime::youki) fn detach_stderr(io: &DetachIo) -> Vec<u8> {
    if let Some(path) = io.stderr.as_deref() {
        if let Ok(bytes) = std::fs::read(path) {
            if !bytes.is_empty() {
                return bytes;
            }
        }
    }
    std::fs::read(&io.youki_log).unwrap_or_default()
}
