// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! S3 bucket-name rules for CreateBucket on the path-style surface.

use crate::multipart::s3_err;
use pistack_types::ApiResult;

/// Reject a name S3 would not accept as `InvalidBucketName`.
///
/// The internal `clean_name` on the create path is looser (it only trims and
/// bans `/`), so the name is checked here before a record is made. A bucket
/// resolved by id is not put through this rule.
pub(crate) fn validate_bucket_name(name: &str) -> ApiResult<()> {
    let bad = || {
        s3_err(
            "InvalidBucketName",
            "Bucket names must be 3-63 characters of lowercase letters, digits, and hyphens, \
             starting and ending with a letter or digit.",
        )
    };
    if name.len() < 3 || name.len() > 63 {
        return Err(bad());
    }
    if !name
        .bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
        return Err(bad());
    }
    let ends_ok = |c: u8| c.is_ascii_lowercase() || c.is_ascii_digit();
    if !ends_ok(name.as_bytes()[0]) || !ends_ok(*name.as_bytes().last().unwrap()) {
        return Err(bad());
    }
    if name.contains("..") || name.contains(".-") || name.contains("-.") {
        return Err(bad());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use pistack_types::error::ErrorCode;

    fn code(name: &str) -> Option<String> {
        validate_bucket_name(name).err().and_then(|e| {
            e.details
                .get("s3_code")
                .and_then(|v| v.as_str().map(str::to_owned))
        })
    }

    #[test]
    fn accepts_s3_legal_names() {
        for name in [
            "abc",
            "my-bucket",
            "a1b2c3",
            "bucket-1",
            &"x".repeat(63),
            "1abc",
            "abc1",
        ] {
            assert!(validate_bucket_name(name).is_ok(), "{name}");
        }
    }

    #[test]
    fn rejects_illegal_names_as_invalid_bucket_name() {
        for name in [
            "",
            "ab",
            &"x".repeat(64),
            "UPPER",
            "has space",
            "-lead",
            "trail-",
            "a..b",
            "a-.b",
            "a_b",
            "under_score",
        ] {
            let err = validate_bucket_name(name).unwrap_err();
            assert_eq!(err.code, ErrorCode::BadRequest, "{name}");
            assert_eq!(code(name).as_deref(), Some("InvalidBucketName"), "{name}");
        }
    }
}
