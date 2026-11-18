// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::Resource;

// ---- kinds and well-known keys ------------------------------------------------

use super::daemon_set_spec::DaemonSetSpec;
use super::daemon_set_status::DaemonSetStatus;

pub type DaemonSet = Resource<DaemonSetSpec, DaemonSetStatus>;
