// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

// ---- kinds and well-known keys ------------------------------------------------

/// Label on a pod naming the ReplicaSet/Job that owns it, and on a
/// ReplicaSet/Job naming the Deployment/CronJob that owns it. Value is the
/// owner's id. (Kubernetes uses `ownerReferences`; a label keeps it
/// selectable.)
pub const OWNER_LABEL: &str = "loom.pistack.io/owner";
