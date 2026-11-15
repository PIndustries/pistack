// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

pub(in crate::probe) fn http_path(path: &str) -> Result<String, ()> {
    let path = if path.is_empty() { "/" } else { path };
    if path.chars().any(|c| c.is_ascii_control() || c == ' ') {
        return Err(());
    }
    if path.starts_with('/') {
        Ok(path.to_owned())
    } else {
        Ok(format!("/{path}"))
    }
}
