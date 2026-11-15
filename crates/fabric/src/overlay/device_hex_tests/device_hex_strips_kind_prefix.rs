// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::overlay::device_hex::device_hex;

#[test]
pub(in crate::overlay) fn device_hex_strips_kind_prefix() {
    assert_eq!(
        device_hex("net-01a10318-a507-74fd-a86e-5acbda974722").unwrap(),
        "01a10318a507"
    );
}
