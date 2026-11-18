// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::Client;
use crate::http::load_bucket;
use crate::s3::caller;
use axum::{
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Extension,
};
use pistack_types::{ApiResult, Caller};

use super::xmlns::XMLNS;

/// The versioning configuration document of a visible bucket.
pub(crate) async fn get_versioning(
    c: Client,
    ext: Option<Extension<Caller>>,
    bucket_name: String,
) -> ApiResult<Response> {
    let who = caller(ext)?;
    let bucket = load_bucket(&c, &who, &bucket_name)?;
    let status = if bucket.spec.versioning {
        "<Status>Enabled</Status>"
    } else {
        ""
    };
    let xml = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
<VersioningConfiguration xmlns=\"{XMLNS}\">{status}</VersioningConfiguration>"
    );
    Ok((
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/xml")],
        xml,
    )
        .into_response())
}
