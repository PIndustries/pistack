// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `Meta`: the metadata every resource carries.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use super::id::Id;

/// Metadata every resource carries.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Meta {
    pub id: Id,
    /// Project (tenant) that owns the resource. `None` only for cloud-scoped
    /// things like hosts and the admin project itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<Id>,
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub labels: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub annotations: BTreeMap<String, String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// Monotonic per-resource version; every write bumps it. Used for
    /// optimistic concurrency (`If-Match`) and watch ordering.
    pub revision: u64,
}

impl Meta {
    pub fn new(kind: &'static str, name: impl Into<String>, project: Option<Id>) -> Self {
        let now = Utc::now();
        Self {
            id: Id::new(kind),
            project,
            name: name.into(),
            description: String::new(),
            labels: BTreeMap::new(),
            annotations: BTreeMap::new(),
            created_at: now,
            updated_at: now,
            revision: 1,
        }
    }
    pub fn touch(&mut self) {
        self.updated_at = Utc::now();
        self.revision += 1;
    }
}
