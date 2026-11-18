// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use percent_encoding::utf8_percent_encode;

use super::build_string_to_sign::build_string_to_sign;
use super::canonical_uri::canonical_uri;
use super::derive_signing_key::derive_signing_key;
use super::form_urlencoded_iter::form_urlencoded_iter;
use super::hmac_sha256_hex::hmac_sha256_hex;
use super::parse_amz_date::parse_amz_date;
use super::parsed_auth::ParsedAuth;
use super::query_value::QUERY_VALUE;
use super::unsigned_payload::UNSIGNED_PAYLOAD;

/// Build a presigned query string (without path) for GET/PUT.
#[allow(clippy::too_many_arguments)]
pub fn presign_query(
    method: &str,
    path: &str,
    host: &str,
    access_key_id: &str,
    secret: &[u8],
    region: &str,
    amz_date: &str,
    expires: i64,
    extra_query: &str,
) -> String {
    let date_stamp: String = amz_date.chars().take(8).collect();
    let credential = format!("{access_key_id}/{date_stamp}/{region}/s3/aws4_request");
    let mut params: Vec<(String, String)> = vec![
        ("X-Amz-Algorithm".into(), "AWS4-HMAC-SHA256".into()),
        ("X-Amz-Credential".into(), credential),
        ("X-Amz-Date".into(), amz_date.to_owned()),
        ("X-Amz-Expires".into(), expires.to_string()),
        ("X-Amz-SignedHeaders".into(), "host".into()),
    ];
    for (k, v) in form_urlencoded_iter(extra_query) {
        params.push((k, v));
    }
    params.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    let canonical_qs = params
        .iter()
        .map(|(k, v)| {
            format!(
                "{}={}",
                utf8_percent_encode(k, QUERY_VALUE),
                utf8_percent_encode(v, QUERY_VALUE)
            )
        })
        .collect::<Vec<_>>()
        .join("&");
    let canonical = format!(
        "{method}\n{}\n{canonical_qs}\nhost:{host}\n\nhost\n{UNSIGNED_PAYLOAD}",
        canonical_uri(path)
    );
    let auth = ParsedAuth {
        access_key_id: access_key_id.to_owned(),
        date_stamp: date_stamp.clone(),
        region: region.to_owned(),
        amz_date: parse_amz_date(amz_date).expect("amz_date"),
        signed_headers: vec!["host".into()],
        signature: String::new(),
        payload_hash: UNSIGNED_PAYLOAD.to_owned(),
        expires_secs: Some(expires),
        presigned: true,
    };
    let sts = build_string_to_sign(&auth, &canonical);
    let key = derive_signing_key(secret, &date_stamp, region, "s3");
    let sig = hmac_sha256_hex(&key, sts.as_bytes());
    format!("{canonical_qs}&X-Amz-Signature={sig}")
}
