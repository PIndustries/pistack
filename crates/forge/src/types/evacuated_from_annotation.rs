// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

/// Set on a machine that an evacuation moved off a host. The value is that
/// host's id. If the host comes back still holding the machine, that copy is
/// stale and the registrar has the agent delete it. slab reads the same key
/// to plug shared volumes back in.
pub const EVACUATED_FROM_ANNOTATION: &str = "pistack.io/evacuated-from";
