// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

/// Most bytes of stdout and of stderr kept from one command probe.
/// The stored message keeps a shorter scrubbed excerpt.
pub const PROBE_EXEC_OUTPUT_CAP: usize = 1024;

pub(in crate::probe) const PROBE_MESSAGE_CAP: usize = 160;

pub(in crate::probe) const DEFAULT_PERIOD: u32 = 10;
pub(in crate::probe) const DEFAULT_TIMEOUT: u32 = 1;
pub(in crate::probe) const DEFAULT_SUCCESS: u32 = 1;
pub(in crate::probe) const DEFAULT_FAILURE: u32 = 3;
