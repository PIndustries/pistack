// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

// ---- kinds and well-known keys ------------------------------------------------

/// Host `node_name` stamped on forge hosts so a DaemonSet can pin one pod
/// per host through `pod.spec.node_selector`.
pub const HOSTNAME_LABEL: &str = "pistack.io/hostname";
