// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::{kind, Kind, Meta, Resource};
use serde::{Deserialize, Serialize};

use super::security_group_spec::SecurityGroupSpec;
use super::security_group_status::SecurityGroupStatus;

resource!(
    SecurityGroup,
    SecurityGroupSpec,
    SecurityGroupStatus,
    kind::SECGROUP
);
