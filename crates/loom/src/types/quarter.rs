// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

// ---- kinds and well-known keys ------------------------------------------------

use super::int_or_percent::IntOrPercent;

pub(crate) fn quarter() -> IntOrPercent {
    IntOrPercent::Percent("25%".into())
}
