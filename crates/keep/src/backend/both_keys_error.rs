// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::backend_err::backend_err;
use super::consts::{META_ROOT_KEY, META_ROOT_KEY_WRAPPED};
use crate::error::{KeepError, Result};

/// The startup refusal for a store that holds both keys. `matched` is the
/// comparison when Vault settings were available to make one.
pub(in crate::backend) fn both_keys_error(matched: Option<Result<bool>>) -> KeepError {
    let finish = "stop the server and run `pistack keep migrate-to-vault` to keep the wrapped \
                  key (it deletes the plaintext copy), or `pistack keep migrate-to-software` to \
                  keep the plaintext one";
    match matched {
        Some(Ok(true)) => backend_err(format!(
            "this store holds the same root key twice, in the clear ({META_ROOT_KEY}) and \
             wrapped by Vault ({META_ROOT_KEY_WRAPPED}), as an interrupted migration would \
             leave it; refusing to start until one is removed: {finish}"
        )),
        Some(Ok(false)) => backend_err(format!(
            "this store holds two different root keys, {META_ROOT_KEY} and \
             {META_ROOT_KEY_WRAPPED}; refusing to use or delete either. Restore the store from \
             a backup taken before the change"
        )),
        Some(Err(e)) => backend_err(format!(
            "this store holds both a software root key ({META_ROOT_KEY}) and a wrapped one \
             ({META_ROOT_KEY_WRAPPED}), and the wrapped one could not be compared: {e}"
        )),
        None => backend_err(format!(
            "this store holds both a software root key ({META_ROOT_KEY}) and a wrapped one \
             ({META_ROOT_KEY_WRAPPED}); refusing to guess which is live. With the Vault \
             settings in the environment, {finish}"
        )),
    }
}
