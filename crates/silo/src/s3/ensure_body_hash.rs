// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::sigv4::{self, ExpectedPayloadSha256};
use axum::extract::Request;
use pistack_types::{ApiError, ApiResult};

/// Read the whole body and check it against the payload hash the SigV4
/// middleware recorded (if any). Used by handlers that take a body they do
/// not otherwise consume, so a declared hash is still honoured.
pub(crate) async fn ensure_body_hash(req: Request) -> ApiResult<()> {
    let expected = req.extensions().get::<ExpectedPayloadSha256>().cloned();
    let body = axum::body::to_bytes(req.into_body(), 1024 * 1024)
        .await
        .map_err(|e| ApiError::bad_request(format!("reading body: {e}")))?;
    sigv4::verify_body_sha256(expected.as_ref(), &body)
}
