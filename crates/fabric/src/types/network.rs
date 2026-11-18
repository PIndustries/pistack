// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::{kind, Kind, Meta, Resource};
use serde::{Deserialize, Serialize};

use super::network_spec::NetworkSpec;
use super::network_status::NetworkStatus;

resource!(Network, NetworkSpec, NetworkStatus, kind::NETWORK);
