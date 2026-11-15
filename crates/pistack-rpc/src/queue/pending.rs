// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use chrono::{DateTime, Utc};
use tokio::sync::oneshot;
use tokio::time::Instant;

use crate::proto::CommandKind;

use super::outcome::Outcome;

pub(crate) struct Pending {
    pub(crate) node: String,
    pub(crate) kind: CommandKind,
    pub(crate) issued_at: DateTime<Utc>,
    pub(crate) enqueued: Instant,
    pub(crate) delivered: bool,
    pub(crate) waiter: Option<oneshot::Sender<Outcome>>,
    /// Meta key of the record, for a keyed command.
    pub(crate) record: Option<String>,
}
