// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::multipart::s3_err;
use pistack_types::ApiResult;

/// The `<Status>` of a VersioningConfiguration body: `true` for `Enabled`,
/// `false` for `Suspended`. Anything else is `MalformedXML`.
pub(crate) fn parse_status(body: &str) -> ApiResult<bool> {
    let bad = || {
        s3_err(
            "MalformedXML",
            "VersioningConfiguration needs a Status of Enabled or Suspended",
        )
    };
    if !body.contains("VersioningConfiguration") {
        return Err(bad());
    }
    let inner = body
        .split_once("<Status>")
        .map(|(_, rest)| rest)
        .ok_or_else(bad)?;
    let status = inner
        .split_once("</Status>")
        .map(|(v, _)| v)
        .ok_or_else(bad)?;
    match status.trim() {
        "Enabled" => Ok(true),
        "Suspended" => Ok(false),
        other => Err(s3_err(
            "MalformedXML",
            format!("Status must be Enabled or Suspended, got '{other}'"),
        )),
    }
}
