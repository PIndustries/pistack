// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::{ApiError, ApiResult};
use std::io;
use std::time::Duration;
use tokio_util::io::StreamReader;

use crate::store::ImageData;

use super::fetch_err::fetch_err;
use super::max_import_bytes::MAX_IMPORT_BYTES;
use super::redirect_action::redirect_action;

/// GET `url`, following at most five redirects when every hop passes
/// [`validate_source_url`]. The body is capped at 8 GiB.
pub(crate) async fn fetch_source(url: &str) -> ApiResult<ImageData> {
    let http = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::custom(redirect_action))
        .connect_timeout(Duration::from_secs(30))
        .read_timeout(Duration::from_secs(120))
        .timeout(Duration::from_secs(2 * 60 * 60))
        .build()
        .map_err(|e| ApiError::internal(format!("image import: {e}")))?;
    let resp = http
        .get(url)
        .send()
        .await
        .map_err(fetch_err)?
        .error_for_status()
        .map_err(fetch_err)?;
    if resp.content_length().is_some_and(|n| n > MAX_IMPORT_BYTES) {
        return Err(ApiError::bad_request("image import exceeds 8 GiB"));
    }
    let stream = futures::stream::unfold((resp, 0u64), |(mut resp, read)| async move {
        match resp.chunk().await {
            Ok(Some(chunk)) => {
                let next = read.saturating_add(chunk.len() as u64);
                if next > MAX_IMPORT_BYTES {
                    Some((
                        Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "image import exceeds 8 GiB",
                        )),
                        (resp, next),
                    ))
                } else {
                    Some((Ok(chunk), (resp, next)))
                }
            }
            Ok(None) => None,
            Err(e) => Some((Err(io::Error::other(e)), (resp, read))),
        }
    });
    Ok(Box::pin(StreamReader::new(stream)))
}
