// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub index: u64,
    pub term: u64,
    pub payload: Vec<u8>,
}
