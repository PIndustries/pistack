// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_store::Store;

use super::KeyBackend;
use crate::backend::backend_err::backend_err;
use crate::backend::both_keys_error::both_keys_error;
use crate::backend::both_keys_match::both_keys_match;
use crate::backend::consts::{ENV_BACKEND, META_ROOT_KEY, META_ROOT_KEY_WRAPPED};
use crate::backend::decode_software::decode_software;
use crate::backend::unwrap::unwrap;
use crate::backend::wrapped_root_key::WrappedRootKey;
use crate::crypto::RootKey;
use crate::error::Result;

impl KeyBackend {
    /// Read the root key when one exists. Never creates one, never calls
    /// Vault to wrap.
    pub fn load_existing(&self, store: &Store) -> Result<Option<RootKey>> {
        // Half the secrets are under one key and half under another: no
        // single key serves this store until the rotation is finished.
        if crate::rekey::in_progress(store)? {
            return Err(crate::rekey::refuse_start());
        }
        let plain = store.get_meta::<String>(META_ROOT_KEY)?;
        let wrapped = store.get_meta::<WrappedRootKey>(META_ROOT_KEY_WRAPPED)?;
        match (self, plain, wrapped) {
            (KeyBackend::VaultTransit(v), Some(b64), Some(w)) => {
                Err(both_keys_error(Some(both_keys_match(v, &b64, &w))))
            }
            (KeyBackend::Software, Some(_), Some(_)) => Err(both_keys_error(None)),
            (_, None, None) => Ok(None),
            (KeyBackend::Software, Some(b64), None) => decode_software(&b64).map(Some),
            (KeyBackend::Software, None, Some(w)) => Err(backend_err(format!(
                "the root key in this store is wrapped by Vault Transit (mount '{}', key '{}'); \
                 set {ENV_BACKEND}=vault with matching settings",
                w.mount, w.key
            ))),
            (KeyBackend::VaultTransit(_), Some(_), None) => Err(backend_err(format!(
                "{ENV_BACKEND}=vault but this store holds a software root key in \
                 {META_ROOT_KEY}; it is neither used nor replaced. To move it into Vault, \
                 stop the server and run `pistack keep migrate-to-vault` with these settings"
            ))),
            (KeyBackend::VaultTransit(v), None, Some(w)) => unwrap(v, &w).map(Some),
        }
    }
}
