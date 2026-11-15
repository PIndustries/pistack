// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

pub struct EnvGuard {
    pub(in crate::runtime) saved: Vec<(&'static str, Option<std::ffi::OsString>)>,
}
