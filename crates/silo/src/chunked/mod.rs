// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! AWS `aws-chunked` body decoding for SigV4 streaming uploads.
//!
//! Supported `x-amz-content-sha256` values:
//! * [`STREAMING_PAYLOAD`] — signed chunks (`STREAMING-AWS4-HMAC-SHA256-PAYLOAD`)
//! * [`STREAMING_UNSIGNED_TRAILER`] — unsigned chunks + trailing checksum
//! * [`STREAMING_SIGNED_TRAILER`] — signed chunks + trailing checksum + trailer signature
//!
//! Identity-review rules:
//! 1. Chunk signatures chain from the Authorization seed signature; compares
//!    are constant-time ([`ct_eq_hex`]).
//! 2. A final zero-length chunk is required and verified; ending without it
//!    yields `IncompleteBody` and nothing is stored.
//! 3. `x-amz-decoded-content-length` must equal the decoded byte count.
//! 4. Each chunk is buffered (capped at [`MAX_CHUNK_BYTES`]), verified, then
//!    yielded — no bytes reach the temp-file writer before that chunk passes.
//! 5. Chunk-header lines and trailer sections are length-capped; CRLF search is
//!    incremental. Trailers wait for a full `\r\n\r\n` before parsing.
//!    Trailing junk after the final chunk/trailer yields `IncompleteBody`.

pub(super) mod checksum_state;
pub(super) mod consts;
pub(super) mod decode_stream;
pub(super) mod decoded_stream;
pub(super) mod find_crlf;
pub(super) mod find_double_crlf;
pub(super) mod finish_trailer;
pub(super) mod frame_signed_chunks;
pub(super) mod frame_signed_trailer;
pub(super) mod frame_unsigned_trailer;
pub(super) mod parse_chunk_header;
pub(super) mod split_header_line;
pub(super) mod streaming_auth;
pub(super) mod take_line_capped;
#[cfg(test)]
pub(super) mod tests;

pub use crate::chunked::checksum_state::chunk_signature::chunk_signature;
pub use crate::chunked::checksum_state::ct_eq_hex::ct_eq_hex;
pub use crate::chunked::checksum_state::trailer_checksum::TrailerChecksum;
pub use crate::chunked::checksum_state::trailer_signature::trailer_signature;
pub use crate::chunked::consts::*;
pub use crate::chunked::decode_stream::decode_stream;
pub use crate::chunked::decoded_stream::decoded_stream::DecodedStream;
pub use crate::chunked::frame_signed_chunks::frame_signed_chunks;
pub use crate::chunked::frame_signed_trailer::frame_signed_trailer;
pub use crate::chunked::frame_unsigned_trailer::frame_unsigned_trailer;
pub use crate::chunked::streaming_auth::streaming_auth::StreamingAuth;
pub use crate::chunked::streaming_auth::streaming_kind::StreamingKind;
