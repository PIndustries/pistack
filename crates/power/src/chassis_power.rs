// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

/// The observed power state of a chassis.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChassisPower {
    On,
    Off,
}
