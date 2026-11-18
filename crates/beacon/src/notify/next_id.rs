// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

pub(crate) fn next_id() -> u16 {
    use rand::Rng;
    rand::thread_rng().gen()
}
