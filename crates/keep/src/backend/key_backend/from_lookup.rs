// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::KeyBackend;
use crate::backend::backend_err::backend_err;
use crate::backend::consts::*;
use crate::backend::vault_from::vault_from;
use crate::error::Result;

impl KeyBackend {
    /// Parse backend settings from `lookup` (an environment). Unset or
    /// empty values count as missing.
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Result<Self> {
        let get = |k: &str| {
            lookup(k)
                .map(|v| v.trim().to_owned())
                .filter(|v| !v.is_empty())
        };
        match get(ENV_BACKEND).as_deref() {
            None | Some("software") => {
                if let Some(var) = VAULT_VARS.iter().find(|v| get(v).is_some()) {
                    return Err(backend_err(format!(
                        "{var} is set but {ENV_BACKEND} is not 'vault'; set {ENV_BACKEND}=vault \
                         or unset the Vault variables"
                    )));
                }
                Ok(KeyBackend::Software)
            }
            Some("vault") => Ok(KeyBackend::VaultTransit(vault_from(&get)?)),
            Some("pkcs11") => Err(backend_err(format!(
                "{ENV_BACKEND}=pkcs11: the PKCS#11 root-key backend is parked and not implemented; \
                 use 'software' or 'vault'"
            ))),
            Some(other) => Err(backend_err(format!(
                "{ENV_BACKEND}={other:?} is not a root-key backend; use software or vault \
                 (pkcs11 is parked)"
            ))),
        }
    }
}
