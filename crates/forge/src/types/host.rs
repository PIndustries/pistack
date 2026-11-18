// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::{kind, Kind, Meta, Resource};
use serde::{Deserialize, Serialize};
use std::ops::{Deref, DerefMut};

use super::host_spec::HostSpec;
use super::host_status::HostStatus;

resource_kind!(
    /// A host the scheduler can place machines on.
    Host,
    HostSpec,
    HostStatus,
    kind::HOST
);
