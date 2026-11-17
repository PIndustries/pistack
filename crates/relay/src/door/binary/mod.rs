// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Finding the publisher's binary. An override environment variable wins
//! over `PATH`; a missing binary is unavailable, never a silent skip.

mod resolve_bin;
mod which;

pub(crate) use resolve_bin::resolve_bin;
pub(crate) use which::which;
