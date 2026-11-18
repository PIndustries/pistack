// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::into_body_read_err::IntoBodyReadErr;
use bytes::Bytes;
use futures::Stream;
use pistack_types::ApiError;

/// Preserve [`ApiError`] stream errors (including `details.s3_code`); wrap others.
pub(crate) fn map_stream_err<S, E>(body: S) -> impl Stream<Item = Result<Bytes, ApiError>> + Send
where
    S: Stream<Item = Result<Bytes, E>> + Send,
    E: IntoBodyReadErr + Send,
{
    use futures::StreamExt;
    body.map(|item| item.map_err(IntoBodyReadErr::into_body_read_err))
}
