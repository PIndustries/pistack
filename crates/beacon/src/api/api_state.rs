// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::Client;
use crate::notify::Notifier;

/// What a handler needs: the write path, and — when this node runs the notify
/// loop — the way to ask it for one.
#[derive(Clone)]
pub struct ApiState {
    pub client: Client,
    pub notifier: Option<Notifier>,
}
