// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use serde::{Deserialize, Serialize};
use std::net::IpAddr;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AllocationPool {
    pub start: IpAddr,
    pub end: IpAddr,
}
