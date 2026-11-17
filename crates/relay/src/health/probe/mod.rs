// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! One probe and its result, bounded by the monitor's timeout.

mod call;
mod result;

pub use call::probe;
pub use result::Probe;
