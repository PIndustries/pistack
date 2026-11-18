// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::{ApiError, ApiResult};
use serde::{Deserialize, Serialize};

/// How the server can cut a host's power from outside it. Set by a cloud
/// admin when a host is worth fencing: without it, evacuating a host that
/// stopped reporting relies on an operator's word that it is dead.
///
/// The password is never here. `secret` is the *name* of a secret in the
/// secrets service that holds it; the payload stays there and is read only
/// at the moment of a power-off, and is never returned or logged.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct PowerControl {
    /// `ipmi` or `redfish`.
    pub kind: String,
    /// The BMC address: a `host[:port]` for IPMI, a URL or host for Redfish.
    pub address: String,
    /// The BMC user.
    pub user: String,
    /// Name (or id) of the secret holding the BMC password. Never the
    /// password itself.
    pub secret: String,
}

impl PowerControl {
    /// A control with its fields trimmed and checked. The kind must be one
    /// the power transport understands; the address, user and secret name
    /// must be non-empty.
    pub fn checked(kind: &str, address: &str, user: &str, secret: &str) -> ApiResult<PowerControl> {
        let kind = power::Kind::parse(kind)?.as_str().to_owned();
        let address = address.trim();
        let user = user.trim();
        let secret = secret.trim();
        if address.is_empty() {
            return Err(ApiError::bad_request("power control needs a bmc address"));
        }
        if user.is_empty() {
            return Err(ApiError::bad_request("power control needs a bmc user"));
        }
        if secret.is_empty() {
            return Err(ApiError::bad_request(
                "power control needs the name of a secret holding the bmc password",
            ));
        }
        Ok(PowerControl {
            kind,
            address: address.to_owned(),
            user: user.to_owned(),
            secret: secret.to_owned(),
        })
    }
}
