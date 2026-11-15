// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

pub struct PreparedProcess {
    pub(in crate::runtime::youki) args: Vec<String>,
    pub(in crate::runtime::youki) env: Vec<String>,
    pub(in crate::runtime::youki) cwd: String,
}
