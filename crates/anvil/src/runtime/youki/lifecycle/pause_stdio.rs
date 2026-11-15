// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::lifecycle::DetachIo;
use std::path::Path;

pub(in crate::runtime::youki) fn pause_stdio(bundle: &Path) -> DetachIo {
    DetachIo {
        stdout: None,
        stderr: None,
        youki_log: bundle.join("youki.log"),
    }
}
