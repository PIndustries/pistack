// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

// ---- kinds and well-known keys ------------------------------------------------

/// Annotation a DELETE handler sets to ask a controller to tear the
/// resource down (the Kubernetes `deletionTimestamp` idea). Same key as
/// forge uses on machines so one convention covers the cloud.
pub const DELETE_ANNOTATION: &str = forge::DELETE_ANNOTATION;
