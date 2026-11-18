// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::multipart::s3_err;
use axum::{
    extract::{Request, State},
    middleware::Next,
    response::Response,
};
use pistack_types::Caller;

use super::authenticate::authenticate;
use super::detect_sigv4_material::detect_sigv4_material;
use super::expected_payload_sha256::ExpectedPayloadSha256;
use super::sig_v4_state::SigV4State;
use super::sigv4_error::sigv4_error;

/// Axum middleware: if no [`Caller`] is present yet and the request carries
/// SigV4 (header or query), verify it via keep and insert `credential.caller`.
///
/// A request that already has a bearer [`Caller`] **and** SigV4 material
/// (AWS4 `Authorization` or presigned `X-Amz-*` query) is rejected — the
/// two schemes must not be combined. Bearer-only requests pass through.
pub async fn middleware(State(state): State<SigV4State>, mut req: Request, next: Next) -> Response {
    let has_caller = req.extensions().get::<Caller>().is_some();
    let sigv4 = match detect_sigv4_material(&req) {
        Ok(v) => v,
        Err(err) => return sigv4_error(err),
    };
    if has_caller && sigv4 {
        return sigv4_error(s3_err(
            "InvalidRequest",
            "cannot combine bearer credentials with SigV4",
        ));
    }
    if has_caller {
        return next.run(req).await;
    }
    match authenticate(&state, &req) {
        Ok(None) => next.run(req).await,
        Ok(Some(auth)) => {
            // The gateway's role check ran before this caller existed, so
            // the same rule is applied here: writes need admin or `member`.
            if !matches!(req.method().as_str(), "GET" | "HEAD" | "OPTIONS")
                && !auth.caller.can_write()
            {
                return sigv4_error(s3_err(
                    "AccessDenied",
                    "this access key's principal may only read",
                ));
            }
            req.extensions_mut().insert(auth.caller);
            if let Some(sha) = auth.expected_payload_sha256 {
                req.extensions_mut().insert(ExpectedPayloadSha256(sha));
            }
            if let Some(streaming) = auth.streaming {
                req.extensions_mut().insert(streaming);
            }
            next.run(req).await
        }
        Err(err) => sigv4_error(err),
    }
}
