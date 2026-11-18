// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

// ---- kinds and well-known keys ------------------------------------------------

/// Annotation the Deployment controller keeps on each ReplicaSet with its
/// monotonically increasing rollout revision (`1`, `2`, ...).
pub const REVISION_ANNOTATION: &str = "loom.pistack.io/revision";
