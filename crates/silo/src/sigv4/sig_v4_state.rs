// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use keep::Client as KeepClient;

/// Shared state for the SigV4 middleware.
#[derive(Clone)]
pub struct SigV4State {
    pub keep: Option<KeepClient>,
    pub region: String,
}
