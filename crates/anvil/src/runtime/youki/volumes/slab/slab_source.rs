// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SlabSource {
    Missing,
    Dir,
    File,
    Block,
    Other,
}
