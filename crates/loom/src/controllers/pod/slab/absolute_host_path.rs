// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
pub(in crate::controllers::pod) fn absolute_host_path(raw: &str) -> Option<&str> {
    if raw.starts_with('/') && !raw.contains('\0') && !raw.split('/').any(|part| part == "..") {
        Some(raw)
    } else {
        None
    }
}
