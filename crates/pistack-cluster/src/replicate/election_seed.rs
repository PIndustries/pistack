// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

/// Mix node identity, attempt, and a clock so each election wait differs.
pub fn election_seed(node_id: &str, attempt: u64, now_nanos: u64) -> u64 {
    let mut seed = attempt
        .wrapping_mul(0x9E3779B97F4A7C15)
        .wrapping_add(now_nanos);
    for byte in node_id.as_bytes() {
        seed = seed.wrapping_mul(16_777_619).wrapping_add(*byte as u64);
    }
    seed
}
