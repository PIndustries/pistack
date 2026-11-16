// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::chunked::checksum_state::checksum_state::ChecksumState;
use crate::chunked::checksum_state::trailer_checksum::TrailerChecksum;

/// Frame unsigned chunks + trailing checksum.
pub fn frame_unsigned_trailer(
    data: &[u8],
    chunk_size: usize,
    checksum: TrailerChecksum,
) -> Vec<u8> {
    let mut out = Vec::new();
    let mut offset = 0;
    let mut state = ChecksumState::new(checksum);
    while offset < data.len() {
        let end = (offset + chunk_size).min(data.len());
        let chunk = &data[offset..end];
        state.update(chunk);
        out.extend_from_slice(format!("{:x}\r\n", chunk.len()).as_bytes());
        out.extend_from_slice(chunk);
        out.extend_from_slice(b"\r\n");
        offset = end;
    }
    let b64 = state.finalize_b64();
    out.extend_from_slice(b"0\r\n");
    out.extend_from_slice(format!("{}:{b64}\r\n\r\n", checksum.header_name()).as_bytes());
    out
}
