// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

#[derive(Clone, Copy)]
pub struct Limits {
    pub(in crate::runtime::youki) vcpus: u32,
    pub(in crate::runtime::youki) memory_mib: u64,
}
