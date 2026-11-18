// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::hex_sha256::hex_sha256;
use super::parsed_auth::ParsedAuth;

pub(crate) fn build_string_to_sign(auth: &ParsedAuth, canonical: &str) -> String {
    let scope = format!("{}/{}/s3/aws4_request", auth.date_stamp, auth.region);
    format!(
        "AWS4-HMAC-SHA256\n{}\n{}\n{}",
        auth.amz_date.format("%Y%m%dT%H%M%SZ"),
        scope,
        hex_sha256(canonical.as_bytes())
    )
}
