// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

pub(crate) fn secret_names(names: &[String]) -> Vec<u8> {
    names.join("\n").into_bytes()
}
