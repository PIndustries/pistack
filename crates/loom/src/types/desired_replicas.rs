// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

// ---- kinds and well-known keys ------------------------------------------------

/// Desired replica count from the latest metric sample.
pub fn desired_replicas(
    current: u32,
    metric_value: f64,
    target_value: f64,
    min_replicas: u32,
    max_replicas: u32,
) -> u32 {
    let lo = min_replicas;
    let hi = max_replicas.max(lo);
    if !metric_value.is_finite() || !target_value.is_finite() || target_value <= 0.0 {
        return current.clamp(lo, hi);
    }
    let base = if current == 0 { 1 } else { current };
    let raw = (base as f64) * metric_value / target_value;
    let desired = if raw.is_finite() {
        raw.ceil().max(0.0) as u32
    } else {
        current
    };
    desired.clamp(lo, hi)
}
