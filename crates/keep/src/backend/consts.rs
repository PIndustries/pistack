// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

pub use crate::crypto::META_ROOT_KEY;
pub use crate::vault::VAULT_TIMEOUT;

/// Store meta key holding the Vault-wrapped root key ([`WrappedRootKey`](super::WrappedRootKey)).
pub const META_ROOT_KEY_WRAPPED: &str = "keep/root_key_wrapped";

pub const ENV_BACKEND: &str = "PISTACK_KEEP_BACKEND";
pub const ENV_VAULT_ADDR: &str = "PISTACK_KEEP_VAULT_ADDR";
pub const ENV_VAULT_TOKEN: &str = "PISTACK_KEEP_VAULT_TOKEN";
pub const ENV_VAULT_TOKEN_FILE: &str = "PISTACK_KEEP_VAULT_TOKEN_FILE";
pub const ENV_VAULT_KEY: &str = "PISTACK_KEEP_VAULT_KEY";
pub const ENV_VAULT_MOUNT: &str = "PISTACK_KEEP_VAULT_MOUNT";
pub const ENV_VAULT_NAMESPACE: &str = "PISTACK_KEEP_VAULT_NAMESPACE";
pub const ENV_VAULT_CACERT: &str = "PISTACK_KEEP_VAULT_CACERT";

pub(in crate::backend) const VAULT_VARS: [&str; 7] = [
    ENV_VAULT_ADDR,
    ENV_VAULT_TOKEN,
    ENV_VAULT_TOKEN_FILE,
    ENV_VAULT_KEY,
    ENV_VAULT_MOUNT,
    ENV_VAULT_NAMESPACE,
    ENV_VAULT_CACERT,
];

pub(in crate::backend) const WRAPPED_BY_VAULT: &str = "vault-transit";
