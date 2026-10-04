// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! A stack is a PiStack resource: a document the user wrote (YAML or JSON
//! text) plus string parameters. It is not a Heat HOT template and not an
//! OpenTofu module. Apply creates network, subnet and machine rows from
//! that document. It does not run a reconciler.

use pistack_types::{kind, ApiError, ApiResult, Resource, Spec};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Status text stored at create time. Apply replaces it with the ids it
/// inserted and any ids that were already present.
pub const ACCEPTED_MESSAGE: &str =
    "accepted; kits are not reconciled yet (no VM or pod side effects)";

/// What the user asked to apply.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct StackSpec {
    /// Name inside the document, independent of the resource name.
    pub name: String,
    /// The user's stack text, YAML or JSON. Stored as given.
    pub document: String,
    #[serde(default)]
    pub parameters: BTreeMap<String, String>,
}

impl Spec for StackSpec {
    const KIND: &'static str = kind::STACK;
}

/// Observed phase. Apply stays at `Accepted`.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum StackPhase {
    #[default]
    Accepted,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct StackStatus {
    pub phase: StackPhase,
    pub message: String,
    /// Ids inserted by the last apply. Already-present names are not listed.
    #[serde(default)]
    pub created: Vec<String>,
}

impl StackStatus {
    pub fn accepted() -> Self {
        Self {
            phase: StackPhase::Accepted,
            message: ACCEPTED_MESSAGE.to_owned(),
            created: Vec::new(),
        }
    }
}

impl Default for StackStatus {
    fn default() -> Self {
        Self::accepted()
    }
}

pub type Stack = Resource<StackSpec, StackStatus>;

/// Reject an empty document. Otherwise accept JSON (`serde_json::Value`) or
/// text that looks like YAML: it contains `:` and does not start with `{`.
pub fn validate_document(document: &str) -> ApiResult<()> {
    let trimmed = document.trim();
    if trimmed.is_empty() {
        return Err(ApiError::bad_request("document must not be empty"));
    }
    if serde_json::from_str::<serde_json::Value>(document).is_ok() {
        return Ok(());
    }
    if trimmed.contains(':') && !trimmed.starts_with('{') {
        return Ok(());
    }
    Err(ApiError::bad_request("document must be JSON or YAML"))
}

/// Name plus document. Trims the name; leaves the document bytes alone.
pub fn validate_spec(spec: &mut StackSpec) -> ApiResult<()> {
    spec.name = spec.name.trim().to_owned();
    if spec.name.is_empty() || spec.name.len() > 128 || spec.name.contains('/') {
        return Err(ApiError::bad_request(
            "spec.name must be 1-128 characters without '/'",
        ));
    }
    validate_document(&spec.document)
}
