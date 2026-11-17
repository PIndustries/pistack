// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::parse_primary::parse_primary;

#[test]
fn primary_addresses() {
    assert_eq!(
        parse_primary("10.0.0.1").unwrap(),
        "10.0.0.1:53".parse().unwrap()
    );
    assert_eq!(
        parse_primary("10.0.0.1:5353").unwrap(),
        "10.0.0.1:5353".parse().unwrap()
    );
    assert_eq!(
        parse_primary("[::1]:53").unwrap(),
        "[::1]:53".parse().unwrap()
    );
    assert!(parse_primary("not-an-ip").is_err());
}
