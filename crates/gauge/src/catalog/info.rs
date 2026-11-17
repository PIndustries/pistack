// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::metric_info::MetricInfo;
use super::table::CATALOG;

/// What PiStack knows about `name`, if it is one of its own.
pub fn info(name: &str) -> Option<&'static MetricInfo> {
    CATALOG.iter().find(|m| m.name == name)
}
