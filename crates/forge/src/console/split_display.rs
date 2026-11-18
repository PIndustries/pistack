// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

pub(crate) fn split_display(line: &str) -> Result<(&str, &str), String> {
    if let Some(number) = line.strip_prefix("[::1]:") {
        return Ok(("::1", number));
    }
    match line.rsplit_once(':') {
        Some((host, number)) if !number.is_empty() => Ok((host, number)),
        _ => Err("bad vnc display".into()),
    }
}
