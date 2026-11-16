// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

pub struct EnvSlot {
    pub(in crate::runtime) key: &'static str,
    pub(in crate::runtime) prev: Option<std::ffi::OsString>,
}
