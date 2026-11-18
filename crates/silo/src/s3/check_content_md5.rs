// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::multipart::{self};
use base64::Engine;
use pistack_types::ApiResult;

/// The body must be the bytes the declared Content-MD5 was taken of. A header
/// that is not 16 bytes of base64 is InvalidDigest; a right-shaped hash of the
/// wrong bytes is BadDigest. Both refuse the whole request.
pub(crate) fn check_content_md5(declared: &str, body: &[u8]) -> ApiResult<()> {
    let declared = declared.trim();
    let digest = base64::engine::general_purpose::STANDARD
        .decode(declared)
        .ok()
        .filter(|d| d.len() == 16);
    let Some(digest) = digest else {
        return Err(multipart::s3_err(
            "InvalidDigest",
            "The Content-MD5 you specified was an invalid.",
        ));
    };
    use md5::Digest;
    if md5::Md5::digest(body).as_slice() != digest {
        return Err(multipart::s3_err(
            "BadDigest",
            "The Content-MD5 you specified did not match what we received.",
        ));
    }
    Ok(())
}
