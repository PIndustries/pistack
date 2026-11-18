// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

/// One collapsed row: either an original item or a rolled-up prefix.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Delimited<T> {
    Item(T),
    CommonPrefix(String),
}
