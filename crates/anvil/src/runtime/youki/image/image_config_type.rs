// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

#[derive(Clone, Debug, Default)]
pub struct ImageConfig {
    pub(in crate::runtime::youki) entrypoint: Vec<String>,
    pub(in crate::runtime::youki) cmd: Vec<String>,
    pub(in crate::runtime::youki) env: Vec<String>,
    pub(in crate::runtime::youki) working_dir: String,
}
