// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::chunked::decoded_stream::decoded_stream::DecodedStream;
use crate::chunked::decoded_stream::phase::Phase;
use crate::multipart::s3_err;
use pistack_types::ApiError;

impl DecodedStream {
    pub(in crate::chunked) fn on_wire_eof(&mut self) -> ApiError {
        self.finished = true;
        match self.phase {
            Phase::Done | Phase::ExpectEof => {
                s3_err("IncompleteBody", "unexpected state after body EOF")
            }
            Phase::NeedHeader | Phase::NeedData { .. } => s3_err(
                "IncompleteBody",
                "aws-chunked body ended before the final zero-length chunk",
            ),
            Phase::NeedTrailer => s3_err(
                "IncompleteBody",
                "aws-chunked body ended before trailing checksum headers",
            ),
        }
    }
}
