// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::exec::valid_env_name;
use pistack_types::ApiError;
use pistack_types::ApiResult;

pub(in crate::runtime::youki) fn validate_exec(
    argv: &[String],
    env: &[(String, String)],
    cwd: Option<&str>,
) -> ApiResult<()> {
    if argv.is_empty() {
        return Err(ApiError::bad_request(
            "youki runtime: exec command is empty",
        ));
    }
    if argv.iter().any(|arg| arg.contains('\0')) {
        return Err(ApiError::bad_request(
            "youki runtime: exec command contains a NUL byte",
        ));
    }
    for (name, value) in env {
        if !valid_env_name(name) || value.contains('\0') || value.contains('\n') {
            return Err(ApiError::bad_request(
                "youki runtime: exec env must be a NAME=value pair",
            ));
        }
    }
    if let Some(dir) = cwd {
        if dir.is_empty() || dir.starts_with('-') || dir.contains('\0') {
            return Err(ApiError::bad_request(
                "youki runtime: exec working directory is not a path",
            ));
        }
    }
    Ok(())
}
