// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::*;

/// AWS published GET Object example (header auth).
#[test]
fn aws_get_object_header_vector() {
    let secret = b"wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY";
    let canonical = "GET\n\
/test.txt\n\
\n\
host:examplebucket.s3.amazonaws.com\n\
range:bytes=0-9\n\
x-amz-content-sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855\n\
x-amz-date:20130524T000000Z\n\
\n\
host;range;x-amz-content-sha256;x-amz-date\n\
e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
    assert_eq!(
        hex_sha256(canonical.as_bytes()),
        "7344ae5b7ee6c3e7e6b0fe0640412a37625d1fbfff95c48bbb2dc43964946972"
    );
    let sts = "AWS4-HMAC-SHA256\n\
20130524T000000Z\n\
20130524/us-east-1/s3/aws4_request\n\
7344ae5b7ee6c3e7e6b0fe0640412a37625d1fbfff95c48bbb2dc43964946972";
    let key = derive_signing_key(secret, "20130524", "us-east-1", "s3");
    assert_eq!(
        hmac_sha256_hex(&key, sts.as_bytes()),
        "f0e8bdb87c964420e857bd35b5d6ed310bd44f0170aba48dd91039c6036bdb41"
    );
    let sig = sign_request(
        "GET",
        "/test.txt",
        "",
        &[
            ("host".into(), "examplebucket.s3.amazonaws.com".into()),
            ("range".into(), "bytes=0-9".into()),
            ("x-amz-content-sha256".into(), EMPTY_PAYLOAD_SHA256.into()),
            ("x-amz-date".into(), "20130524T000000Z".into()),
        ],
        EMPTY_PAYLOAD_SHA256,
        "AKIAIOSFODNN7EXAMPLE",
        secret,
        "us-east-1",
        "20130524T000000Z",
    );
    assert_eq!(
        sig,
        "f0e8bdb87c964420e857bd35b5d6ed310bd44f0170aba48dd91039c6036bdb41"
    );
}

#[test]
fn aws_presigned_get_vector() {
    let secret = b"wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY";
    let qs = "X-Amz-Algorithm=AWS4-HMAC-SHA256&X-Amz-Credential=AKIAIOSFODNN7EXAMPLE%2F20130524%2Fus-east-1%2Fs3%2Faws4_request&X-Amz-Date=20130524T000000Z&X-Amz-Expires=86400&X-Amz-SignedHeaders=host";
    let canonical = format!(
        "GET\n/test.txt\n{qs}\nhost:examplebucket.s3.amazonaws.com\n\nhost\nUNSIGNED-PAYLOAD"
    );
    assert_eq!(
        hex_sha256(canonical.as_bytes()),
        "3bfa292879f6447bbcda7001decf97f4a54dc650c8942174ae0a9121cf58ad04"
    );
    let got = presign_query(
        "GET",
        "/test.txt",
        "examplebucket.s3.amazonaws.com",
        "AKIAIOSFODNN7EXAMPLE",
        secret,
        "us-east-1",
        "20130524T000000Z",
        86400,
        "",
    );
    assert!(got.ends_with(
        "X-Amz-Signature=aeeed9bbccd4d02ee5c0109b86d86835f995330da4c265957d157751f604d404"
    ));
}

#[test]
fn aws4_scheme_requires_trailing_space() {
    assert!(is_aws4_authorization(
        "AWS4-HMAC-SHA256 Credential=x/20200101/pistack/s3/aws4_request, Signature=ab"
    ));
    assert!(!is_aws4_authorization("AWS4-HMAC-SHA256garbage"));
    assert!(!is_aws4_authorization("AWS4-HMAC-SHA256"));
    assert!(!is_aws4_authorization("Bearer x"));
}

#[test]
fn canonical_uri_encodes_segments() {
    assert_eq!(canonical_uri("/bucket/my key"), "/bucket/my%20key");
    assert_eq!(canonical_uri("/"), "/");
    assert_eq!(canonical_uri("/a/b"), "/a/b");
}
