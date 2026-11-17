// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::metric_info::MetricInfo;
use super::metric_kind::MetricKind;

pub(crate) const fn gauge(name: &'static str, help: &'static str) -> MetricInfo {
    MetricInfo {
        name,
        kind: MetricKind::Gauge,
        help,
    }
}

pub(crate) const fn counter(name: &'static str, help: &'static str) -> MetricInfo {
    MetricInfo {
        name,
        kind: MetricKind::Counter,
        help,
    }
}
