// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! A stack is a PiStack resource: a document the user wrote (YAML or JSON
//! text) plus string parameters. Apply creates network, subnet, machine,
//! router, pod, listener, and record set rows from that document, and
//! updates the rows an earlier apply of the same stack created. Each
//! successful stack apply records the spec as a revision; rollback puts an
//! earlier revision back and applies it. A kit is a stored document applied
//! the same way. A workflow applies named stacks in order and stops on the
//! first error. Apply does not run a reconciler.

use chrono::{DateTime, Utc};
use pistack_types::{kind, ApiError, ApiResult, Resource, Spec};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Status text stored when a stack or kit is created. Apply replaces it
/// with the ids it inserted, updated, or found already present.
pub const ACCEPTED_MESSAGE: &str =
    "accepted; apply creates network, subnet, machine, router, pod, listener, and record set rows";

/// Status text stored while an apply is realizing the document.
pub const APPLYING_MESSAGE: &str = "applying the document";

/// Applied stack specs kept for rollback. Older revisions are dropped.
pub const REVISION_HISTORY_LIMIT: usize = 10;

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

/// Observed phase. `Accepted` until the first apply. Apply moves to
/// `Applying` while it writes rows, then to `Ready` or `Failed`.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum StackPhase {
    #[default]
    Accepted,
    Applying,
    Ready,
    Failed,
}

/// One applied spec. Rollback re-applies `spec` as a new revision.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct StackRevision {
    pub revision: u64,
    pub spec: StackSpec,
    pub applied_at: DateTime<Utc>,
    /// Set when a rollback produced this revision: the revision it restored.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restored_from: Option<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct StackStatus {
    pub phase: StackPhase,
    pub message: String,
    /// Ids inserted by the last apply. Already-present names are not listed.
    #[serde(default)]
    pub created: Vec<String>,
    /// Ids whose spec the last apply changed.
    #[serde(default)]
    pub updated: Vec<String>,
    /// Managed ids the last successful apply's document no longer names.
    /// They are kept, not deleted.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub left: Vec<String>,
    /// Every id an apply of this resource inserted. Later applies update
    /// these rows when the document changes; rows it did not insert are
    /// left as they are.
    #[serde(default)]
    pub managed: Vec<String>,
    /// Revision of the last successful apply. `0` before the first one.
    #[serde(default)]
    pub revision: u64,
    /// Applied specs, oldest first, at most [`REVISION_HISTORY_LIMIT`].
    /// Only stacks keep history.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub history: Vec<StackRevision>,
}

impl StackStatus {
    pub fn accepted() -> Self {
        Self {
            phase: StackPhase::Accepted,
            message: ACCEPTED_MESSAGE.to_owned(),
            created: Vec::new(),
            updated: Vec::new(),
            left: Vec::new(),
            managed: Vec::new(),
            revision: 0,
            history: Vec::new(),
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

/// Observed phase for a kit. Same phases as a stack; a kit keeps no
/// revision history.
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
