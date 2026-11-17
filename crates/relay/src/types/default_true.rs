// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The serde default for an `admin_up` field, which is up unless told
//! otherwise.

pub(super) fn default_true() -> bool {
    true
}
