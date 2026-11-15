// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::lifecycle::open_append;
use pistack_types::ApiResult;
use std::path::Path;
use std::process::Stdio;

pub(in crate::runtime::youki) fn stdio_target(path: Option<&Path>) -> ApiResult<Stdio> {
    match path {
        None => Ok(Stdio::null()),
        Some(path) => Ok(Stdio::from(open_append(path)?)),
    }
}
