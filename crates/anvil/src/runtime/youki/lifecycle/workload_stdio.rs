// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::lifecycle::DetachIo;
use std::path::Path;

pub(in crate::runtime::youki) fn workload_stdio(bundle: &Path) -> DetachIo {
    let log = bundle.join("log");
    DetachIo {
        stdout: Some(log.clone()),
        stderr: Some(log),
        youki_log: bundle.join("youki.log"),
    }
}
