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
async fn duplicate_checksum_trailer_rejected() {
    let payload = b"dup";
    let mut framed = frame_unsigned_trailer(payload, 8, TrailerChecksum::Crc32);
    // Inject a second checksum line before the blank terminator.
    // framed ends with: 0\r\nname:b64\r\n\r\n
    assert!(framed.ends_with(b"\r\n\r\n"));
    framed.truncate(framed.len() - 2); // drop final \r\n of terminator
    let name = TrailerChecksum::Crc32.header_name();
    framed.extend_from_slice(format!("{name}:AAAAAA==\r\n\r\n").as_bytes());
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
        Some("InvalidRequest")
    );
    assert!(e.message.contains("duplicate"));
}
