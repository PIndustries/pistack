// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

pub struct OciBind {
    pub(in crate::runtime::youki) destination: String,
    pub(in crate::runtime::youki) source: String,
    pub(in crate::runtime::youki) read_only: bool,
    pub(in crate::runtime::youki) directory: bool,
}
