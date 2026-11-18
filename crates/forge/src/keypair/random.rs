// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use rand::RngCore;

pub(crate) fn random(n: usize) -> Vec<u8> {
    let mut out = vec![0u8; n];
    rand::thread_rng().fill_bytes(&mut out);
    out
}
