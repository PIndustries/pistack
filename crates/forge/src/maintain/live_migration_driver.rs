// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

/// The host capability that can live-migrate a guest. The agent's migrate
/// command is `virsh migrate`, whatever else the host runs, and it is
/// reached only when the agent reports this capability.
pub const LIVE_MIGRATION_DRIVER: &str = "libvirt";
