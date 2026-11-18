// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

#[derive(Debug, Default)]
pub(crate) struct Rejections {
    pub(crate) not_ready: usize,
    pub(crate) disabled: usize,
    pub(crate) kind: usize,
    pub(crate) selector: usize,
    pub(crate) resources: usize,
}
