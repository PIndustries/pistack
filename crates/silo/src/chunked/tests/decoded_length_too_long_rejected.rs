// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::chunked::decode_stream::decode_stream;
use crate::chunked::frame_signed_chunks::frame_signed_chunks;
use crate::chunked::streaming_auth::streaming_auth::StreamingAuth;
use crate::chunked::streaming_auth::streaming_kind::StreamingKind;
use crate::sigv4::derive_signing_key;
use bytes::Bytes;
use futures::StreamExt;

#[tokio::test]
async fn decoded_length_too_long_rejected() {
    let secret = b"wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY";
    let key = derive_signing_key(secret, "20130524", "us-east-1", "s3");
    let seed = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
    let amz_date = "20130524T000000Z";
    let scope = "20130524/us-east-1/s3/aws4_request";
    let data = b"hello-world-extra";
    let (framed, _) = frame_signed_chunks(data, 64, &key, amz_date, scope, seed);
    let auth = StreamingAuth {
        kind: StreamingKind::Signed,
        seed_signature: seed.into(),
        signing_key: key,
        amz_date: amz_date.into(),
        scope: scope.into(),
        decoded_content_length: 4, // too short declared
    };
    let stream = futures::stream::once(async move { Ok::<_, String>(Bytes::from(framed)) });
    let mut decoded = decode_stream(stream, auth);
    let mut err = None;
    while let Some(item) = decoded.next().await {
        if let Err(e) = item {
            err = Some(e);
            break;
        }
    }
    let e = err.expect("must error");
    assert_eq!(
        e.details.get("s3_code").and_then(|v| v.as_str()),
        Some("IncompleteBody")
    );
}
