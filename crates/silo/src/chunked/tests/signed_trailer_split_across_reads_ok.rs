// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::chunked::checksum_state::trailer_checksum::TrailerChecksum;
use crate::chunked::decode_stream::decode_stream;
use crate::chunked::frame_signed_trailer::frame_signed_trailer;
use crate::chunked::streaming_auth::streaming_auth::StreamingAuth;
use crate::chunked::streaming_auth::streaming_kind::StreamingKind;
use crate::sigv4::derive_signing_key;
use bytes::Bytes;
use futures::StreamExt;

#[tokio::test]
async fn signed_trailer_split_across_reads_ok() {
    let secret = b"wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY";
    let key = derive_signing_key(secret, "20130524", "us-east-1", "s3");
    let seed = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let amz_date = "20130524T000000Z";
    let scope = "20130524/us-east-1/s3/aws4_request";
    let payload = b"signed-split-trailer";
    let framed = frame_signed_trailer(
        payload,
        5,
        &key,
        amz_date,
        scope,
        seed,
        TrailerChecksum::Sha256,
    );
    let pieces: Vec<Result<Bytes, String>> =
        framed.iter().map(|&b| Ok(Bytes::from(vec![b]))).collect();
    let auth = StreamingAuth {
        kind: StreamingKind::SignedTrailer {
            checksum: TrailerChecksum::Sha256,
        },
        seed_signature: seed.into(),
        signing_key: key,
        amz_date: amz_date.into(),
        scope: scope.into(),
        decoded_content_length: payload.len() as u64,
    };
    let mut decoded = decode_stream(futures::stream::iter(pieces), auth);
    let mut got = Vec::new();
    while let Some(item) = decoded.next().await {
        got.extend_from_slice(&item.expect("ok"));
    }
    assert_eq!(&got[..], payload);
}
