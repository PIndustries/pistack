// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::hmac_sha256::hmac_sha256;

pub fn derive_signing_key(secret: &[u8], date: &str, region: &str, service: &str) -> Vec<u8> {
    let mut key = Vec::with_capacity(4 + secret.len());
    key.extend_from_slice(b"AWS4");
    key.extend_from_slice(secret);
    let k_date = hmac_sha256(&key, date.as_bytes());
    let k_region = hmac_sha256(&k_date, region.as_bytes());
    let k_service = hmac_sha256(&k_region, service.as_bytes());
    hmac_sha256(&k_service, b"aws4_request")
}
