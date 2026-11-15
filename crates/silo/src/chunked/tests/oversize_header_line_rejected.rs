// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::chunked::consts::MAX_CHUNK_HEADER_BYTES;
use crate::chunked::decode_stream::decode_stream;
use crate::chunked::streaming_auth::streaming_auth::StreamingAuth;
use crate::chunked::streaming_auth::streaming_kind::StreamingKind;
use bytes::Bytes;
use futures::StreamExt;

#[tokio::test]
async fn oversize_header_line_rejected() {
    let auth = StreamingAuth {
        kind: StreamingKind::Signed,
        seed_signature: "aa".repeat(32),
        signing_key: vec![0; 32],
        amz_date: "20130524T000000Z".into(),
        scope: "20130524/us-east-1/s3/aws4_request".into(),
        decoded_content_length: 0,
    };
    let line = "a".repeat(MAX_CHUNK_HEADER_BYTES + 1);
    let stream =
        futures::stream::once(async move { Ok::<_, String>(Bytes::from(line.into_bytes())) });
    let mut decoded = decode_stream(stream, auth);
    let e = decoded.next().await.unwrap().unwrap_err();
    assert_eq!(
        e.details.get("s3_code").and_then(|v| v.as_str()),
        Some("InvalidRequest")
    );
}
