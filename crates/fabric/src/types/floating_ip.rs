// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::{kind, Kind, Meta, Resource};
use serde::{Deserialize, Serialize};

use super::floating_ip_spec::FloatingIpSpec;
use super::floating_ip_status::FloatingIpStatus;

resource!(
    FloatingIp,
    FloatingIpSpec,
    FloatingIpStatus,
    kind::FLOATING_IP
);
