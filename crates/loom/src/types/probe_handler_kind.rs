// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

// ---- kinds and well-known keys ------------------------------------------------

/// Which action a [`Probe`] will run. `Exec` runs its command inside the
/// container on youki. Other runtimes report that probe unsupported.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProbeHandlerKind {
    Http,
    Tcp,
    Exec,
    Missing,
}
