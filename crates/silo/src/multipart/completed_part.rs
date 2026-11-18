// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

#[derive(Clone, Debug)]
pub struct CompletedPart {
    pub part_number: u32,
    pub etag: String,
}
