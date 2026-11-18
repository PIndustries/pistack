// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use axum::{
    body::Body,
    extract::{Path, State},
    http::header,
    response::{IntoResponse, Response},
    Extension,
};
use pistack_types::{ApiResult, Caller};
use tokio_util::io::ReaderStream;

use crate::client::Client;

use super::caller::caller;
use super::internal::internal;

pub(crate) async fn download(
    State(c): State<Client>,
    ext: Option<Extension<Caller>>,
    Path(id): Path<String>,
) -> ApiResult<Response> {
    let img = c.get_visible(&caller(ext)?, &id)?;
    let data = c.open_image(&img).await?;
    let mut resp = Body::from_stream(ReaderStream::with_capacity(data, 256 * 1024)).into_response();
    let h = resp.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        "application/octet-stream".parse().map_err(internal)?,
    );
    if let Some(n) = img.status.size_bytes {
        h.insert(
            header::CONTENT_LENGTH,
            n.to_string().parse().map_err(internal)?,
        );
    }
    if let Some(s) = &img.status.sha256 {
        h.insert("x-image-sha256", s.parse().map_err(internal)?);
    }
    h.insert(
        header::CONTENT_DISPOSITION,
        format!(
            "attachment; filename=\"{}.{}\"",
            img.meta.name, img.spec.format
        )
        .parse()
        .map_err(internal)?,
    );
    Ok(resp)
}
