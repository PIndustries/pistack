// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use hmac::Hmac;
use sha2::Sha256;

pub(in crate::chunked::checksum_state) type HmacSha256 = Hmac<Sha256>;
