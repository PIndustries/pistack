// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::{kind, Kind, Meta, Resource};
use serde::{Deserialize, Serialize};

use super::subnet_spec::SubnetSpec;
use super::subnet_status::SubnetStatus;

resource!(Subnet, SubnetSpec, SubnetStatus, kind::SUBNET);
