// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::Spec;
use serde::{Deserialize, Serialize};

use super::default_content_type::default_content_type;
use super::kind_object::KIND_OBJECT;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ObjectSpec {
    pub bucket: pistack_types::Id,
    pub key: String,
    #[serde(default = "default_content_type")]
    pub content_type: String,
}

impl Spec for ObjectSpec {
    const KIND: &'static str = KIND_OBJECT;
}
