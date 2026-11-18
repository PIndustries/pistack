// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::multipart::s3_err;
use pistack_types::ApiResult;

pub(crate) fn check_region(got: &str, want: &str) -> ApiResult<()> {
    if got != want {
        return Err(s3_err(
            "AuthorizationHeaderMalformed",
            format!("Region '{got}' does not match configured region '{want}'"),
        ));
    }
    Ok(())
}
