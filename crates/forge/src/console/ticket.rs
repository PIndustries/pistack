// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::time::Instant;

pub(crate) struct Ticket {
    pub(crate) machine_id: String,
    pub(crate) expires: Instant,
}
