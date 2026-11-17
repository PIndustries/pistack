// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! What edge publish we already queued for a listener, so a re-reconcile
//! that changes nothing does not enqueue a second command.

pub(crate) struct EdgePublish {
    pub(crate) key: String,
}
