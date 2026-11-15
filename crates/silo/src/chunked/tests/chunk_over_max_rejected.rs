// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::chunked::checksum_state::trailer_checksum::TrailerChecksum;
use crate::chunked::consts::MAX_CHUNK_BYTES;
use crate::chunked::decode_stream::decode_stream;
use crate::chunked::streaming_auth::streaming_auth::StreamingAuth;
use crate::chunked::streaming_auth::streaming_kind::StreamingKind;
use bytes::Bytes;
use futures::StreamExt;

#[tokio::test]
async fn chunk_over_max_rejected() {
    let auth = StreamingAuth {
        kind: StreamingKind::UnsignedTrailer {
            checksum: TrailerChecksum::Crc32,
        },
        seed_signature: String::new(),
        signing_key: Vec::new(),
        amz_date: "20130524T000000Z".into(),
        scope: "20130524/us-east-1/s3/aws4_request".into(),
        decoded_content_length: 0,
    };
    let huge = format!("{:x}\r\n", MAX_CHUNK_BYTES + 1);
    let stream =
        futures::stream::once(async move { Ok::<_, String>(Bytes::from(huge.into_bytes())) });
    let mut decoded = decode_stream(stream, auth);
    let e = decoded.next().await.unwrap().unwrap_err();
    assert_eq!(
        e.details.get("s3_code").and_then(|v| v.as_str()),
        Some("InvalidRequest")
    );
}
