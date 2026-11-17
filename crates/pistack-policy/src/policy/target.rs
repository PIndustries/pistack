// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `Target`: the object an action is applied to.

use std::collections::BTreeMap;

use pistack_types::Id;

/// The object an action is applied to. For `create` and `list`, `project`
/// is the project scope of the request and `owner` is usually `None`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Target {
    pub project: Option<Id>,
    pub owner: Option<Id>,
    pub labels: BTreeMap<String, String>,
}

impl Target {
    /// A cloud-scoped target (no project, no owner): hosts, flavors, the
    /// policy document itself.
    pub fn cloud() -> Self {
        Self::default()
    }
    /// A target inside `project`.
    pub fn project(project: Id) -> Self {
        Self {
            project: Some(project),
            ..Self::default()
        }
    }
    pub fn with_owner(mut self, owner: Id) -> Self {
        self.owner = Some(owner);
        self
    }
    pub fn with_label(mut self, k: impl Into<String>, v: impl Into<String>) -> Self {
        self.labels.insert(k.into(), v.into());
        self
    }
}
