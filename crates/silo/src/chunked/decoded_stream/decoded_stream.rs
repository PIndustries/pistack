// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::chunked::checksum_state::checksum_state::ChecksumState;
use crate::chunked::decoded_stream::phase::Phase;
use crate::chunked::streaming_auth::streaming_auth::StreamingAuth;
use bytes::{Bytes, BytesMut};
use futures::Stream;
use pistack_types::ApiError;
use std::pin::Pin;

pub struct DecodedStream {
    pub(in crate::chunked) body: Pin<Box<dyn Stream<Item = Result<Bytes, ApiError>> + Send>>,
    pub(in crate::chunked) auth: StreamingAuth,
    pub(in crate::chunked) buf: BytesMut,
    pub(in crate::chunked) prev_sig: String,
    pub(in crate::chunked) decoded: u64,
    pub(in crate::chunked) phase: Phase,
    pub(in crate::chunked) checksum: Option<ChecksumState>,
    pub(in crate::chunked) pending: Option<Bytes>,
    pub(in crate::chunked) finished: bool,
    /// Incremental CRLF / trailer-terminator scan offset.
    pub(in crate::chunked) scan_from: usize,
}
