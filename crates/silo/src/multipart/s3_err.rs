// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::error::ErrorCode;
use pistack_types::ApiError;

/// Attach an S3 error code in `details.s3_code` for the HTTP layer.
pub fn s3_err(code: &str, message: impl Into<String>) -> ApiError {
    let message = message.into();
    let status_code = match code {
        "NoSuchUpload" | "NoSuchKey" | "NoSuchBucket" | "NoSuchVersion" => {
            ApiError::not_found(message)
        }
        "EntityTooLarge"
        | "EntityTooSmall"
        | "InvalidPart"
        | "InvalidPartOrder"
        | "InvalidArgument"
        | "InvalidBucketName"
        | "InvalidRequest"
        | "InvalidRange"
        | "MalformedXML"
        | "AuthorizationHeaderMalformed"
        | "IncompleteBody"
        | "BadDigest"
        | "InvalidDigest"
        | "XAmzContentSHA256Mismatch"
        | "XAmzContentChecksumMismatch" => ApiError::bad_request(message),
        "InvalidAccessKeyId"
        | "SignatureDoesNotMatch"
        | "AccessDenied"
        | "RequestTimeTooSkewed" => ApiError::forbidden(message),
        // CreateBucket on a name owned by another project.
        "BucketAlreadyExists" => ApiError::conflict(message),
        // DeleteBucket on a bucket that still holds objects or uploads.
        "BucketNotEmpty" => ApiError::conflict(message),
        // GetObject on a key whose current generation is a delete marker.
        "MethodNotAllowed" => ApiError::new(ErrorCode::MethodNotAllowed, message),
        // S3 uses HTTP 412; pistack ErrorCode has no 412, so Conflict (409).
        "PreconditionFailed" => ApiError::conflict(message),
        "NotImplemented" => ApiError::not_implemented(message),
        _ => ApiError::internal(message),
    };
    status_code.with_detail("s3_code", code)
}
