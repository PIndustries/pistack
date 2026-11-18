// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

/// One listener this process opened, or started to open.
pub(crate) struct Holding {
    /// The owner label (service or ingress) it was built for.
    pub(crate) owner: &'static str,
    /// Programmed here and seen ready. Sockets live in the process that
    /// bound them, so a balancer another server (or an earlier run) marked
    /// ready is programmed here once before it counts as live.
    pub(crate) ready: bool,
}
