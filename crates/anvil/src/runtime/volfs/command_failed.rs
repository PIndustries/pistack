// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::brief::brief;
use super::privilege_hint::privilege_hint;
use pistack_types::ApiError;

pub(in crate::runtime::volfs) fn command_failed(
    name: &str,
    output: &std::process::Output,
) -> ApiError {
    ApiError::unavailable(format!(
        "{name} failed: {}{}",
        brief(output),
        privilege_hint()
    ))
}
