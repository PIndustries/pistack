// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::backend_err::backend_err;
use super::wrapped_root_key::WrappedRootKey;
use crate::error::KeepError;

/// Explain the lock-out after `min_decryption_version` was raised past the
/// version the root key is wrapped with.
pub(in crate::backend) fn too_old_hint(e: KeepError, w: &WrappedRootKey) -> KeepError {
    let msg = e.to_string();
    if !(msg.contains("too old") || msg.contains("disallowed by policy")) {
        return e;
    }
    let n = w.key_version().unwrap_or(0);
    backend_err(format!(
        "{msg}. The root key is wrapped with Transit key version v{n}, below the key's \
         min_decryption_version. Lower min_decryption_version to {n}, stop the server, run \
         `pistack keep rewrap`, then raise it again"
    ))
}
