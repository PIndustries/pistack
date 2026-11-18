// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::collections::BTreeMap;

use super::build_string_to_sign::build_string_to_sign;
use super::canonical_query::canonical_query;
use super::canonical_uri::canonical_uri;
use super::collapse_ws::collapse_ws;
use super::derive_signing_key::derive_signing_key;
use super::hmac_sha256_hex::hmac_sha256_hex;
use super::parse_amz_date::parse_amz_date;
use super::parsed_auth::ParsedAuth;

/// Helper used by tests and clients: sign a request the same way the server verifies.
#[allow(clippy::too_many_arguments)]
pub fn sign_request(
    method: &str,
    path: &str,
    query: &str,
    headers: &[(String, String)],
    payload_hash: &str,
    access_key_id: &str,
    secret: &[u8],
    region: &str,
    amz_date: &str,
) -> String {
    let date_stamp: String = amz_date.chars().take(8).collect();
    let mut header_map: BTreeMap<String, String> = BTreeMap::new();
    for (k, v) in headers {
        header_map
            .entry(k.to_ascii_lowercase())
            .and_modify(|e| {
                e.push(',');
                e.push_str(&collapse_ws(v));
            })
            .or_insert_with(|| collapse_ws(v));
    }
    let signed: Vec<String> = header_map.keys().cloned().collect();
    let mut canonical = String::new();
    canonical.push_str(method);
    canonical.push('\n');
    canonical.push_str(&canonical_uri(path));
    canonical.push('\n');
    // For signing (not presign verification strip), include all query params.
    let auth = ParsedAuth {
        access_key_id: access_key_id.to_owned(),
        date_stamp: date_stamp.clone(),
        region: region.to_owned(),
        amz_date: parse_amz_date(amz_date).expect("amz_date"),
        signed_headers: signed.clone(),
        signature: String::new(),
        payload_hash: payload_hash.to_owned(),
        expires_secs: None,
        presigned: false,
    };
    let cq = canonical_query(if query.is_empty() { None } else { Some(query) }, false);
    canonical.push_str(&cq);
    canonical.push('\n');
    for name in &signed {
        canonical.push_str(name);
        canonical.push(':');
        canonical.push_str(header_map.get(name).map(String::as_str).unwrap_or(""));
        canonical.push('\n');
    }
    canonical.push('\n');
    canonical.push_str(&signed.join(";"));
    canonical.push('\n');
    canonical.push_str(payload_hash);
    let sts = build_string_to_sign(&auth, &canonical);
    let key = derive_signing_key(secret, &date_stamp, region, "s3");
    hmac_sha256_hex(&key, sts.as_bytes())
}
