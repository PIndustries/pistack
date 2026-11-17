// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `StackSpec`: what the user asked to apply.
use pistack_types::{kind, Spec};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use super::parameter_decl::ParameterDecl;

/// What the user asked to apply.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct StackSpec {
    /// Name inside the document, independent of the resource name.
    pub name: String,
    /// The user's stack text, YAML or JSON. Stored as given, references and
    /// all; a value written `{{ param.NAME }}` is substituted at apply.
    pub document: String,
    /// Parameters the document may name.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub declared: Vec<ParameterDecl>,
    #[serde(default)]
    pub parameters: BTreeMap<String, String>,
}

impl Spec for StackSpec {
    const KIND: &'static str = kind::STACK;
}
