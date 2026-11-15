// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::chunked::checksum_state::chunk_signature::chunk_signature;
use crate::chunked::decode_stream::decode_stream;
use crate::chunked::streaming_auth::streaming_auth::StreamingAuth;
use crate::chunked::streaming_auth::streaming_kind::StreamingKind;
use crate::sigv4::derive_signing_key;
use bytes::Bytes;
use futures::StreamExt;

#[tokio::test]
async fn truncated_body_incomplete() {
    let secret = b"wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY";
    let key = derive_signing_key(secret, "20130524", "us-east-1", "s3");
    let seed = "abcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcd";
    let amz_date = "20130524T000000Z";
    let scope = "20130524/us-east-1/s3/aws4_request";
    let data = b"truncate-me";
    let sig = chunk_signature(&key, amz_date, scope, seed, data);
    // Header only — no data, no final chunk.
    let body = format!("{:x};chunk-signature={sig}\r\n", data.len()).into_bytes();
    let auth = StreamingAuth {
        kind: StreamingKind::Signed,
        seed_signature: seed.into(),
        signing_key: key,
        amz_date: amz_date.into(),
        scope: scope.into(),
        decoded_content_length: data.len() as u64,
    };
    let stream = futures::stream::once(async move { Ok::<_, String>(Bytes::from(body)) });
    let mut decoded = decode_stream(stream, auth);
    let e = decoded.next().await.unwrap().unwrap_err();
    assert_eq!(
        e.details.get("s3_code").and_then(|v| v.as_str()),
        Some("IncompleteBody")
    );
}
