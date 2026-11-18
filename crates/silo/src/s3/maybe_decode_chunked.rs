// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::chunked::{self, StreamingAuth};
use crate::multipart::{self};
use futures::StreamExt;
use pistack_types::ApiResult;

use super::decoded_body_stream::DecodedBodyStream;

/// If SigV4 middleware attached [`StreamingAuth`], decode aws-chunked framing
/// into a plain object/part byte stream (verify-before-write happens inside).
pub(crate) fn maybe_decode_chunked<S, E>(
    body: S,
    streaming: Option<StreamingAuth>,
) -> ApiResult<DecodedBodyStream>
where
    S: futures::Stream<Item = Result<bytes::Bytes, E>> + Send + 'static,
    E: std::fmt::Display + Send + 'static,
{
    Ok(match streaming {
        Some(auth) => chunked::decode_stream(body, auth).boxed(),
        None => body
            .map(|item| {
                item.map_err(|e| multipart::s3_err("IncompleteBody", format!("reading body: {e}")))
            })
            .boxed(),
    })
}
