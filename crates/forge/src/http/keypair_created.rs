// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::Keypair;
use serde::Serialize;

/// The create response: the stored resource, and for a generated pair the
/// private key, which is never stored.
#[derive(Debug, Serialize)]
pub struct KeypairCreated {
    #[serde(flatten)]
    pub keypair: Keypair,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub private_key: Option<String>,
}
