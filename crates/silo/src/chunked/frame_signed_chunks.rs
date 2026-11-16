// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::chunked::checksum_state::chunk_signature::chunk_signature;

/// Helper for tests: frame plain bytes as signed aws-chunked.
pub fn frame_signed_chunks(
    data: &[u8],
    chunk_size: usize,
    signing_key: &[u8],
    amz_date: &str,
    scope: &str,
    seed_signature: &str,
) -> (Vec<u8>, String) {
    let mut out = Vec::new();
    let mut prev = seed_signature.to_ascii_lowercase();
    let mut offset = 0;
    while offset < data.len() {
        let end = (offset + chunk_size).min(data.len());
        let chunk = &data[offset..end];
        let sig = chunk_signature(signing_key, amz_date, scope, &prev, chunk);
        out.extend_from_slice(format!("{:x};chunk-signature={sig}\r\n", chunk.len()).as_bytes());
        out.extend_from_slice(chunk);
        out.extend_from_slice(b"\r\n");
        prev = sig;
        offset = end;
    }
    let sig0 = chunk_signature(signing_key, amz_date, scope, &prev, b"");
    out.extend_from_slice(format!("0;chunk-signature={sig0}\r\n").as_bytes());
    out.extend_from_slice(b"\r\n");
    (out, sig0)
}
