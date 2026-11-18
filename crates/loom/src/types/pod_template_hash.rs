// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

// ---- kinds and well-known keys ------------------------------------------------

/// Label the Deployment controller stamps on every ReplicaSet it creates and
/// on the pods under it. Same key as Kubernetes.
pub const POD_TEMPLATE_HASH: &str = "pod-template-hash";
