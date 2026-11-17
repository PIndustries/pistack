// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `Event`: emitted on the bus whenever a resource changes.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::event_action::EventAction;
use super::id::Id;

/// An event emitted on the bus whenever a resource changes.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Event {
    pub kind: String,
    pub id: Id,
    pub project: Option<Id>,
    pub action: EventAction,
    pub revision: u64,
    pub at: DateTime<Utc>,
}
