// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The serde default for a route path: match everything.

pub(super) fn root_path() -> String {
    "/".into()
}
