// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::chunked::checksum_state::trailer_checksum::TrailerChecksum;
use crate::chunked::decode_stream::decode_stream;
use crate::chunked::frame_unsigned_trailer::frame_unsigned_trailer;
use crate::chunked::streaming_auth::streaming_auth::StreamingAuth;
use crate::chunked::streaming_auth::streaming_kind::StreamingKind;
use bytes::Bytes;
use futures::StreamExt;

#[tokio::test]
async fn trailer_split_across_reads_ok() {
    let payload = b"split-trailer";
    let framed = frame_unsigned_trailer(payload, 4, TrailerChecksum::Crc32);
    // Deliver one byte at a time.
    let pieces: Vec<Result<Bytes, String>> =
        framed.iter().map(|&b| Ok(Bytes::from(vec![b]))).collect();
    let auth = StreamingAuth {
        kind: StreamingKind::UnsignedTrailer {
            checksum: TrailerChecksum::Crc32,
        },
        seed_signature: String::new(),
        signing_key: Vec::new(),
        amz_date: "20130524T000000Z".into(),
        scope: "20130524/us-east-1/s3/aws4_request".into(),
        decoded_content_length: payload.len() as u64,
    };
    let mut decoded = decode_stream(futures::stream::iter(pieces), auth);
    let mut got = Vec::new();
    while let Some(item) = decoded.next().await {
        got.extend_from_slice(&item.expect("ok"));
    }
    assert_eq!(&got[..], payload);
}
