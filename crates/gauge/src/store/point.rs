// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use chrono::{DateTime, Utc};

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Point {
    pub(crate) at: DateTime<Utc>,
    pub(crate) value: f64,
}
