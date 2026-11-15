// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::proto::{CommandId, CommandKind};

/// Lifecycle of one command, for `pending()` introspection.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PendingInfo {
    pub id: CommandId,
    pub node: String,
    pub kind: CommandKind,
    pub issued_at: DateTime<Utc>,
    pub delivered: bool,
}
