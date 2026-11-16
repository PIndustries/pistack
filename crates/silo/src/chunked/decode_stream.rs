// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::chunked::checksum_state::checksum_state::ChecksumState;
use crate::chunked::decoded_stream::decoded_stream::DecodedStream;
use crate::chunked::decoded_stream::phase::Phase;
use crate::chunked::streaming_auth::streaming_auth::StreamingAuth;
use crate::multipart::s3_err;
use bytes::{Bytes, BytesMut};
use futures::Stream;

/// Decode a wire `aws-chunked` body into verified plain object bytes.
pub fn decode_stream<S, E>(body: S, auth: StreamingAuth) -> DecodedStream
where
    S: Stream<Item = Result<Bytes, E>> + Send + 'static,
    E: std::fmt::Display + Send + 'static,
{
    use futures::StreamExt;
    let body = body
        .map(|item| item.map_err(|e| s3_err("IncompleteBody", format!("reading body: {e}"))))
        .boxed();
    let checksum = auth.trailer_checksum().map(ChecksumState::new);
    let prev_sig = auth.seed_signature.clone();
    DecodedStream {
        body,
        auth,
        buf: BytesMut::new(),
        prev_sig,
        decoded: 0,
        phase: Phase::NeedHeader,
        checksum,
        pending: None,
        finished: false,
        scan_from: 0,
    }
}
