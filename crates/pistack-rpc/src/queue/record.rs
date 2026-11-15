// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::proto::{CommandId, CommandKind};

/// What the store keeps for one keyed command. The node and the key are in
/// the meta key; the payload is not kept.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Record {
    pub(crate) id: CommandId,
    pub(crate) kind: CommandKind,
    pub(crate) issued_at: DateTime<Utc>,
}
