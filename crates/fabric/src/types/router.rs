// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::{kind, Kind, Meta, Resource};
use serde::{Deserialize, Serialize};

use super::router_spec::RouterSpec;
use super::router_status::RouterStatus;

resource!(Router, RouterSpec, RouterStatus, kind::ROUTER);
