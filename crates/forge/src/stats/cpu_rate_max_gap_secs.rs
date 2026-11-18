// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

/// A CPU reading older than this is not compared with: the machine may
/// have been away, and the average would say nothing about now.
pub(crate) const CPU_RATE_MAX_GAP_SECS: f64 = 300.0;
