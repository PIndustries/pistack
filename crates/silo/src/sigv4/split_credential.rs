// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::multipart::s3_err;
use pistack_types::ApiResult;

pub(crate) fn split_credential(credential: &str) -> ApiResult<(String, String, String)> {
    // access_key_id/date/region/service/aws4_request
    let parts: Vec<&str> = credential.split('/').collect();
    if parts.len() != 5 || parts[3] != "s3" || parts[4] != "aws4_request" {
        return Err(s3_err(
            "AuthorizationHeaderMalformed",
            "Credential must be <id>/<date>/<region>/s3/aws4_request",
        ));
    }
    if parts[1].len() != 8 || !parts[1].bytes().all(|b| b.is_ascii_digit()) {
        return Err(s3_err(
            "AuthorizationHeaderMalformed",
            "Credential date must be YYYYMMDD",
        ));
    }
    Ok((
        parts[0].to_owned(),
        parts[1].to_owned(),
        parts[2].to_owned(),
    ))
}
