// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

/// Annotation a DELETE handler sets to ask the reconciler to tear a machine
/// down. Handlers never write `status`, so the request travels in metadata
/// (the Kubernetes `deletionTimestamp` idea) and the reconciler flips the
/// phase to `Deleting`.
pub const DELETE_ANNOTATION: &str = "pistack.io/delete";
