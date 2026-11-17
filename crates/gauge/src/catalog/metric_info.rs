// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::metric_kind::MetricKind;

pub struct MetricInfo {
    pub name: &'static str,
    pub kind: MetricKind,
    pub help: &'static str,
}
