// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::sync::Arc;

use crate::queue::inner::Inner;

/// Per-node command queues with pending/ack tracking. Clone freely; all
/// clones share one queue.
#[derive(Clone)]
pub struct CommandQueue(pub(crate) Arc<Inner>);
