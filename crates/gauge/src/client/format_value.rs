// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

pub(crate) fn format_value(value: f64) -> String {
    if value.is_nan() {
        return "NaN".into();
    }
    if value.is_infinite() {
        return if value.is_sign_positive() {
            "+Inf".into()
        } else {
            "-Inf".into()
        };
    }
    format!("{value}")
}
