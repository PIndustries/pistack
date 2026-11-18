// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::validate_source_url;

#[test]
fn source_url_allows_public_http_and_rejects_ssrf() {
    assert!(validate_source_url("https://cloud-images.example/x.img").is_ok());
    assert!(validate_source_url("http://example.com:8080/a").is_ok());
    for bad in [
        "ftp://x/y",
        "http://localhost/x",
        "http://127.0.0.1/x",
        "http://127.0.0.1:9/latest",
        "http://[::1]/x",
        "http://[::1]:80/x",
        "http://::1/",
        "http://169.254.169.254/latest",
        "http://user:pass@example.com/x",
        "http://example.com/a b",
        "/etc/passwd",
        "https://",
    ] {
        assert!(validate_source_url(bad).is_err(), "{bad}");
    }
}
