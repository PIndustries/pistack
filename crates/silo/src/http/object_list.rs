// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use serde::Serialize;

use super::listed_object::ListedObject;

#[derive(Serialize)]
pub(crate) struct ObjectList {
    pub(crate) keys: Vec<String>,
    pub(crate) items: Vec<ListedObject>,
    pub(crate) total: usize,
}
