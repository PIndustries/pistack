// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The refusal a follower gives an S3 client, in the shape the object store
//! uses for every error it answers: `<Error><Code>…</Code><Message>…`, at
//! HTTP 503, with a code an S3 client retries on.
//!
//! A follower that cannot carry a body has to say so to a client that reads
//! neither the JSON envelope nor a `details.leader` field, so it borrows
//! silo's own rendering rather than inventing a second one.

use axum::{
    http::header,
    response::{IntoResponse, Response},
};

/// The code for "try the same request again, later, somewhere that can take
/// it". S3 clients retry this one and say the message when they give up.
const SLOW_DOWN: &str = "SlowDown";

/// A 503 an S3 client can read, naming the server that would hold the write.
pub(crate) fn s3_slow_down(leader: &str, limit: usize) -> Response {
    let message = format!(
        "this server cannot pass on a write larger than {}; send it to the leader at {leader}",
        silo::format_byte_limit(limit)
    );
    let xml = silo::s3::error_xml(SLOW_DOWN, &message);
    (
        axum::http::StatusCode::SERVICE_UNAVAILABLE,
        [(header::CONTENT_TYPE, "application/xml")],
        xml,
    )
        .into_response()
}

/// True when the body the client declared is more than a follower will hold.
/// A request that declared no length is not refused here: the carry reads
/// what arrives and answers on its own terms if it runs past the limit.
pub(crate) fn s3_body_too_large(headers: &axum::http::HeaderMap, limit: usize) -> bool {
    match headers.get(header::CONTENT_LENGTH) {
        Some(value) => value
            .to_str()
            .ok()
            .and_then(|text| text.trim().parse::<u64>().ok())
            .is_some_and(|len| len > limit as u64),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderMap;

    fn with_length(text: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::CONTENT_LENGTH,
            text.parse()
                .unwrap_or_else(|_| panic!("{text} is not a header")),
        );
        headers
    }

    #[test]
    fn only_a_body_larger_than_the_limit_is_refused_up_front() {
        let limit = 8 * 1024 * 1024;
        assert!(s3_body_too_large(&with_length("8388609"), limit));
        assert!(!s3_body_too_large(&with_length("8388608"), limit));
        assert!(!s3_body_too_large(&with_length("0"), limit));
        // A length that is not a number is the leader's to complain about.
        assert!(!s3_body_too_large(&with_length("bogus"), limit));
        // A client that declared no length is carried and measured there.
        assert!(!s3_body_too_large(&HeaderMap::new(), limit));
    }

    #[tokio::test]
    async fn a_refusal_is_s3_xml_at_503_naming_the_leader() {
        let response = s3_slow_down("http://10.0.0.11:8443", 8 * 1024 * 1024);
        assert_eq!(
            response.status(),
            axum::http::StatusCode::SERVICE_UNAVAILABLE
        );
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            "application/xml"
        );
        let body = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        let text = String::from_utf8(body.to_vec()).unwrap();
        assert!(text.contains("<Code>SlowDown</Code>"), "{text}");
        assert!(text.contains("http://10.0.0.11:8443"), "{text}");
        assert!(text.contains("8 MiB"), "{text}");
    }
}
