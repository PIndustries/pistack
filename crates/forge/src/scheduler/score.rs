// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Score {
    pub(crate) affinity: i64,
    pub(crate) memory: i128,
    pub(crate) vcpus: u32,
}
