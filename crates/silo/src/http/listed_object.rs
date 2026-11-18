// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::Id;
use serde::Serialize;

#[derive(Serialize)]
pub(crate) struct ListedObject {
    pub(crate) id: Id,
    pub(crate) key: String,
    pub(crate) size_bytes: u64,
    pub(crate) sha256: String,
    pub(crate) version: u64,
}
