// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::Resource;

// ---- kinds and well-known keys ------------------------------------------------

use super::stateful_set_spec::StatefulSetSpec;
use super::stateful_set_status::StatefulSetStatus;

pub type StatefulSet = Resource<StatefulSetSpec, StatefulSetStatus>;
