// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! A stack is a PiStack resource: a document the user wrote (YAML or JSON
//! text) plus string parameters. Apply creates network, subnet, machine,
//! router, pod, listener, and record set rows from that document. A kit is
//! a stored document applied the same way. A workflow applies named stacks
//! in order and stops on the first error. Apply does not run a reconciler.

use pistack_types::{kind, ApiError, ApiResult, Resource, Spec};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Status text stored when a stack or kit is created. Apply replaces it
/// with the ids it inserted and any ids that were already present.
pub const ACCEPTED_MESSAGE: &str =
    "accepted; apply creates network, subnet, machine, router, pod, listener, and record set rows";

/// Status text stored when a workflow is created. Apply replaces it with
/// the stack names it ran.
pub const WORKFLOW_ACCEPTED_MESSAGE: &str =
    "accepted; apply runs each named stack in order and stops on the first error";

/// Id prefix and store table for a kit.
pub const KIT_KIND: &str = "kit";
/// Id prefix and store table for a workflow.
pub const WORKFLOW_KIND: &str = "workflow";

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

/// A stored document. Apply runs the same realize path as a stack.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct KitSpec {
    /// Name inside the document, independent of the resource name.
    pub name: String,
    /// The user's stack text, YAML or JSON. Stored as given.
    pub document: String,
    #[serde(default)]
    pub parameters: BTreeMap<String, String>,
}

impl Spec for KitSpec {
    const KIND: &'static str = KIT_KIND;
}

/// Observed phase for a kit. Apply stays at `Accepted`.
pub type KitStatus = StackStatus;

pub type Kit = Resource<KitSpec, KitStatus>;

/// An ordered list of stack names in one project.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkflowSpec {
    /// Name inside the spec, independent of the resource name.
    #[serde(default)]
    pub name: String,
    /// Stack names or ids, applied in this order.
    #[serde(default)]
    pub stacks: Vec<String>,
}

impl Spec for WorkflowSpec {
    const KIND: &'static str = WORKFLOW_KIND;
}

/// Observed phase. Apply sets `Failed` when a stack returns an error.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowPhase {
    #[default]
    Accepted,
    Failed,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkflowStatus {
    pub phase: WorkflowPhase,
    pub message: String,
    /// Stack names applied by the last run before it stopped.
    #[serde(default)]
    pub applied: Vec<String>,
}

impl WorkflowStatus {
    pub fn accepted() -> Self {
        Self {
            phase: WorkflowPhase::Accepted,
            message: WORKFLOW_ACCEPTED_MESSAGE.to_owned(),
            applied: Vec::new(),
        }
    }
}

impl Default for WorkflowStatus {
    fn default() -> Self {
        Self::accepted()
    }
}

pub type Workflow = Resource<WorkflowSpec, WorkflowStatus>;

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

fn validate_resource_name(name: &str) -> ApiResult<()> {
    if name.is_empty() || name.len() > 128 || name.contains('/') {
        return Err(ApiError::bad_request(
            "spec.name must be 1-128 characters without '/'",
        ));
    }
    Ok(())
}

/// Name plus document. Trims the name; leaves the document bytes alone.
pub fn validate_spec(spec: &mut StackSpec) -> ApiResult<()> {
    spec.name = spec.name.trim().to_owned();
    validate_resource_name(&spec.name)?;
    validate_document(&spec.document)
}

/// Name plus document. Same rules as a stack.
pub fn validate_kit(spec: &mut KitSpec) -> ApiResult<()> {
    spec.name = spec.name.trim().to_owned();
    validate_resource_name(&spec.name)?;
    validate_document(&spec.document)
}

/// Name plus the ordered stack list. Each entry is trimmed.
pub fn validate_workflow(spec: &mut WorkflowSpec) -> ApiResult<()> {
    spec.name = spec.name.trim().to_owned();
    validate_resource_name(&spec.name)?;
    for name in &mut spec.stacks {
        *name = name.trim().to_owned();
        if name.is_empty() || name.len() > 128 || name.contains('/') {
            return Err(ApiError::bad_request(
                "workflow stack names must be 1-128 characters without '/'",
            ));
        }
    }
    Ok(())
}
