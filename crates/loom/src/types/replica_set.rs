// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::Resource;

// ---- kinds and well-known keys ------------------------------------------------

use super::replica_set_spec::ReplicaSetSpec;
use super::replica_set_status::ReplicaSetStatus;

pub type ReplicaSet = Resource<ReplicaSetSpec, ReplicaSetStatus>;
