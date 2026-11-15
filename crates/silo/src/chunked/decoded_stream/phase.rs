// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

pub(in crate::chunked) enum Phase {
    NeedHeader,
    NeedData {
        size: usize,
        declared_sig: Option<String>,
        chunk_buf: Vec<u8>,
    },
    NeedTrailer,
    /// Framing complete; any further body bytes are an error.
    ExpectEof,
    Done,
}
