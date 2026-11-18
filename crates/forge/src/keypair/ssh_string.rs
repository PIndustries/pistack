// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

pub(crate) fn ssh_string(field: &[u8]) -> Vec<u8> {
    let mut out = (field.len() as u32).to_be_bytes().to_vec();
    out.extend_from_slice(field);
    out
}
