// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

#[derive(Clone, Debug, Default)]
pub struct Launch {
    pub(in crate::runtime::youki) image: String,
    pub(in crate::runtime::youki) command: Vec<String>,
    pub(in crate::runtime::youki) args: Vec<String>,
    pub(in crate::runtime::youki) env: Vec<(String, String)>,
    pub(in crate::runtime::youki) cwd: String,
}
