// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Strategy {
    /// Prefer the host with the most free memory.
    #[default]
    Spread,
    /// Prefer the fullest host that still fits (bin-packing).
    Pack,
}
