// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::backend_err::backend_err;
use super::consts::{ENV_VAULT_KEY, ENV_VAULT_MOUNT, WRAPPED_BY_VAULT};
use super::wrapped_root_key::WrappedRootKey;
use crate::error::Result;
use crate::vault::VaultTransit;

pub(in crate::backend) fn check_names(v: &VaultTransit, w: &WrappedRootKey) -> Result<()> {
    if w.backend != WRAPPED_BY_VAULT {
        return Err(backend_err(format!(
            "wrapped root key names backend {:?}; this build only unwraps {WRAPPED_BY_VAULT}",
            w.backend
        )));
    }
    if w.mount != v.mount || w.key != v.key {
        return Err(backend_err(format!(
            "root key was wrapped with Transit mount '{}' key '{}' but {ENV_VAULT_MOUNT}/{ENV_VAULT_KEY} \
             say '{}' / '{}'",
            w.mount, w.key, v.mount, v.key
        )));
    }
    Ok(())
}
