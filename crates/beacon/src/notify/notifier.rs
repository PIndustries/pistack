// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use tokio::sync::mpsc;

/// The primary side: a queue of apices to re-notify. Handing one in never
/// blocks and never fails; a stopped notifier drops it.
#[derive(Clone)]
pub struct Notifier {
    pub(crate) tx: mpsc::UnboundedSender<String>,
}

impl Notifier {
    pub fn new() -> (Self, mpsc::UnboundedReceiver<String>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (Self { tx }, rx)
    }

    /// Ask for `apex` to be notified, whatever its serial says.
    pub fn notify_zone(&self, apex: &str) {
        let _ = self.tx.send(apex.to_owned());
    }
}
