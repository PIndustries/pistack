// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `KitSpec`: a stored document, and the `Kit` resource.
use pistack_types::Spec;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use super::kit_kind::KIT_KIND;
use super::parameter_decl::ParameterDecl;
use super::stack_document::StackDocument;
use super::stack_status::StackStatus;
use pistack_types::Resource;

/// A stored document. Apply runs the same realize path as a stack.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct KitSpec {
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

impl Spec for KitSpec {
    const KIND: &'static str = KIT_KIND;
}

impl StackDocument for KitSpec {
    fn document(&self) -> &str {
        &self.document
    }
    fn declared(&self) -> &[ParameterDecl] {
        &self.declared
    }
    fn values(&self) -> &BTreeMap<String, String> {
        &self.parameters
    }
    fn declared_mut(&mut self) -> &mut Vec<ParameterDecl> {
        &mut self.declared
    }
}

/// Observed phase for a kit. Same phases as a stack; a kit keeps no
/// revision history.
pub type KitStatus = StackStatus;

pub type Kit = Resource<KitSpec, KitStatus>;
