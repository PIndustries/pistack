// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

/// `{a, b}`, OVN's spelling of a list inside an option value.
pub(in crate::backend::ovn) fn braces(items: impl IntoIterator<Item = String>) -> String {
    format!("{{{}}}", items.into_iter().collect::<Vec<_>>().join(", "))
}
