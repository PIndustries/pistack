// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::backend_err::backend_err;
use crate::error::Result;

pub(in crate::backend) fn check_path_part(var: &str, v: &str, allow_slash: bool) -> Result<()> {
    let ok = !v.is_empty()
        && v.split('/')
            .all(|seg| !seg.is_empty() && seg != "." && seg != "..")
        && v.chars().all(|c| {
            c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') || (allow_slash && c == '/')
        });
    if ok {
        Ok(())
    } else {
        Err(backend_err(format!(
            "{var}={v:?} must be [A-Za-z0-9._-]{}",
            if allow_slash {
                " segments joined by '/'"
            } else {
                ""
            }
        )))
    }
}
