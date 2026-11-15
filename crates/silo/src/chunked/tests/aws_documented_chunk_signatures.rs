// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::chunked::checksum_state::chunk_signature::chunk_signature;
use crate::chunked::checksum_state::ct_eq_hex::ct_eq_hex;
use crate::sigv4::derive_signing_key;

/// AWS documented example (65_536 + 1_024 'a's).
#[test]
fn aws_documented_chunk_signatures() {
    let secret = b"wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY";
    let key = derive_signing_key(secret, "20130524", "us-east-1", "s3");
    let seed = "4f232c4386841ef735655705268965c44a0e4690baa4adea153f7db9fa80a0a9";
    let amz_date = "20130524T000000Z";
    let scope = "20130524/us-east-1/s3/aws4_request";

    let c1 = vec![b'a'; 65536];
    let sig1 = chunk_signature(&key, amz_date, scope, seed, &c1);
    assert_eq!(
        sig1,
        "ad80c730a21e5b8d04586a2213dd63b9a0e99e0e2307b0ade35a65485a288648"
    );

    let c2 = vec![b'a'; 1024];
    let sig2 = chunk_signature(&key, amz_date, scope, &sig1, &c2);
    assert_eq!(
        sig2,
        "0055627c9e194cb4542bae2aa5492e3c1575bbb81b612b7d234b86a503ef5497"
    );

    let sig0 = chunk_signature(&key, amz_date, scope, &sig2, b"");
    assert_eq!(
        sig0,
        "b6c6ea8a5354eaf15b3cb7646744f4275b71ea724fed81ceb9323e279d449df9"
    );
    assert!(ct_eq_hex(&sig0, &sig0));
    assert!(!ct_eq_hex(&sig0, &sig1));
}
