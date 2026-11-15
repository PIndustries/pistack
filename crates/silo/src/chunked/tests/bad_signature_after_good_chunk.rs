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
async fn bad_signature_after_good_chunk() {
    let secret = b"wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY";
    let key = derive_signing_key(secret, "20130524", "us-east-1", "s3");
    let seed = "1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef";
    let amz_date = "20130524T000000Z";
    let scope = "20130524/us-east-1/s3/aws4_request";
    let c1 = b"good-chunk-data!!";
    let c2 = b"bad-next";
    let sig1 = chunk_signature(&key, amz_date, scope, seed, c1);
    let mut body = Vec::new();
    body.extend_from_slice(format!("{:x};chunk-signature={sig1}\r\n", c1.len()).as_bytes());
    body.extend_from_slice(c1);
    body.extend_from_slice(b"\r\n");
    body.extend_from_slice(
        format!(
            "{:x};chunk-signature={}\r\n",
            c2.len(),
            "0000000000000000000000000000000000000000000000000000000000000000"
        )
        .as_bytes(),
    );
    body.extend_from_slice(c2);
    body.extend_from_slice(b"\r\n");
    let auth = StreamingAuth {
        kind: StreamingKind::Signed,
        seed_signature: seed.into(),
        signing_key: key,
        amz_date: amz_date.into(),
        scope: scope.into(),
        decoded_content_length: (c1.len() + c2.len()) as u64,
    };
    let stream = futures::stream::once(async move { Ok::<_, String>(Bytes::from(body)) });
    let mut decoded = decode_stream(stream, auth);
    let first = decoded.next().await.expect("good chunk").expect("ok");
    assert_eq!(&first[..], c1);
    let second = decoded.next().await.expect("item");
    assert_eq!(
        second
            .unwrap_err()
            .details
            .get("s3_code")
            .and_then(|v| v.as_str()),
        Some("SignatureDoesNotMatch")
    );
}
