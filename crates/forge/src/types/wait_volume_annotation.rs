// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

/// While this annotation is non-empty, forge does not place or start the
/// machine. Loom sets it to the wait message until every slab volume the
/// pod names is attached on a node.
pub const WAIT_VOLUME_ANNOTATION: &str = "loom.pistack.io/wait-volume";
