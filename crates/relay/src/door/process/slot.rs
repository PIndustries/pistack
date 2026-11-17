// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The publisher process this node started for one listener.

pub(crate) struct Slot {
    pub(crate) argv: Vec<String>,
    pub(crate) child: Option<tokio::process::Child>,
    pub(crate) one_shot: bool,
}
