// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::Client;
use crate::sigv4::SigV4State;

/// Router state: silo client plus optional keep client for SigV4.
#[derive(Clone)]
pub struct AppState {
    pub client: Client,
    pub sigv4: SigV4State,
}
