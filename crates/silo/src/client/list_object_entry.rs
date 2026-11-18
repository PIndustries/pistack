// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::*;

/// One row in a ListObjectsV2 page (object or rolled-up prefix).
#[derive(Clone, Debug, PartialEq)]
pub enum ListObjectEntry {
    Object(Box<Object>),
    CommonPrefix(String),
}

impl ListObjectEntry {
    pub fn sort_key(&self) -> &str {
        match self {
            Self::Object(o) => o.spec.key.as_str(),
            Self::CommonPrefix(p) => p.as_str(),
        }
    }
}
