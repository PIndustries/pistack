// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::Client;
use crate::quota::KeepConfig;

use super::client::client;

pub(in crate::client) fn limited(secrets: u64, certificates: u64) -> Client {
    client().with_limits(KeepConfig {
        max_secrets_per_project: secrets,
        max_certificates_per_project: certificates,
        ..KeepConfig::default()
    })
}
