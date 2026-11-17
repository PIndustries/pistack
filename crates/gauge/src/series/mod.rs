// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! A series is a metric name and a set of labels, written as Prometheus writes it.

mod escape;
mod sanitize;
mod series_fn;
mod series_type;
#[cfg(test)]
mod tests;

pub use sanitize::sanitize_name;
pub use series_fn::series;
pub use series_type::Series;
