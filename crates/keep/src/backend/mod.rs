// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Where the root key comes from.
//!
//! [`RootKey`](crate::crypto::RootKey) is the in-memory AES-256-GCM data key that seals every
//! secret payload and private key. A [`KeyBackend`] decides how that key is
//! kept between restarts:
//!
//! | `PISTACK_KEEP_BACKEND` | Status | Root key at rest |
//! | --- | --- | --- |
//! | unset or `software` | default | base64 in store meta [`META_ROOT_KEY`] |
//! | `vault` | done; not yet run in the lab against a production Vault | wrapped by Vault Transit in [`META_ROOT_KEY_WRAPPED`] |
//! | `pkcs11` | parked | refused at startup: not implemented |
//!
//! The Vault backend wraps and unwraps the key for real (see `vault.rs`).
//! It is tested against a mock Transit server in CI, and an ignored test
//! (`real_vault_migrate_rewrap_and_back`) runs the same paths against a real
//! Vault such as `vault server -dev`. Not yet done: a lab run against a
//! production Vault (TLS, namespace, a least-privilege token).
//!
//! The token needs `update` on `<mount>/encrypt/<key>`,
//! `<mount>/decrypt/<key>` and, for `pistack keep rewrap`,
//! `<mount>/rewrap/<key>`. Only token auth is supported, and keep does not
//! renew the token: it is used at startup and by the commands below, so a
//! token that has expired makes the next start fail with Vault's error.
//!
//! ## Operator commands
//!
//! These run offline with `pistack keep …` while the server is stopped
//! (the store file is locked while a server has it open). They use the same
//! `PISTACK_KEEP_*` environment as the server, and each one changes only
//! this node's store, so run them on every control-plane node.
//!
//! * [`migrate_to_vault`]: wrap the existing software key with Transit,
//!   check that Vault unwraps it to the same bytes, then write
//!   [`META_ROOT_KEY_WRAPPED`] and delete [`META_ROOT_KEY`] in **one store
//!   transaction**. A crash before the commit leaves the software key
//!   untouched; after it, only the wrapped key exists. There is never a
//!   moment with neither key.
//! * [`migrate_to_software`]: the reverse, also one transaction. It puts the
//!   key back in the clear, so the command asks for an explicit flag.
//! * [`rewrap`]: after `vault write -f transit/keys/<key>/rotate`, move the
//!   stored ciphertext to the latest key version with Transit's rewrap
//!   endpoint, checking it still unwraps to the same key. One meta write;
//!   the old ciphertext stays valid until `min_decryption_version` is
//!   raised, so a crash at any point is harmless. Rewrap before raising
//!   `min_decryption_version`.
//! * [`root_key_state`]: what the store holds, for `pistack keep status`.
//! * [`rotate_root_key`](crate::rekey::rotate_root_key): seal every payload
//!   again under a new software root key. See `rekey.rs`.
//!
//! If a store ever holds both keys (for example after hand edits or a
//! partial restore), startup refuses and explains how to finish with the
//! commands above. A store that holds sealed secrets or CAs but no root key
//! is refused too, rather than given a fresh key that cannot open them.
//!
//! Vault settings, all read from the environment:
//!
//! * `PISTACK_KEEP_VAULT_ADDR`: `https://…`; `http://` only to a loopback host
//! * `PISTACK_KEEP_VAULT_TOKEN` or `PISTACK_KEEP_VAULT_TOKEN_FILE` (exactly one)
//! * `PISTACK_KEEP_VAULT_KEY`: Transit key name (required)
//! * `PISTACK_KEEP_VAULT_MOUNT`: Transit mount, default `transit`
//! * `PISTACK_KEEP_VAULT_NAMESPACE`: optional `X-Vault-Namespace`
//! * `PISTACK_KEEP_VAULT_CACERT`: optional PEM file to trust for Vault's TLS
//!
//! Everything fails closed. A bad setting, an unreachable or refusing
//! Vault, or a store whose key was written by the other backend is an error
//! that names the problem. There is no fallback from one backend to another.

mod backend_err;
mod cache;
mod cache_key;
mod check_addr;
mod check_names;
mod check_path_part;
mod configured;
mod consts;
mod decode_software;
mod key_backend;
mod migrate_to_software;
mod migrate_to_vault;
mod migrated;
mod refuse_if_sealed_data;
mod rewrap;
mod rewrapped;
mod root_key_state;
mod too_old_hint;
mod unwrap;
mod unwrap_uncached;
mod vault_from;
mod wrapped_root_key;

mod both_keys_error;
mod both_keys_match;

#[cfg(test)]
pub(crate) use cache::forget_unwrapped;
pub use configured::configured;
pub use consts::{
    ENV_BACKEND, ENV_VAULT_ADDR, ENV_VAULT_CACERT, ENV_VAULT_KEY, ENV_VAULT_MOUNT,
    ENV_VAULT_NAMESPACE, ENV_VAULT_TOKEN, ENV_VAULT_TOKEN_FILE, META_ROOT_KEY,
    META_ROOT_KEY_WRAPPED,
};
pub use key_backend::*;
pub use migrate_to_software::migrate_to_software;
pub use migrate_to_vault::migrate_to_vault;
pub use migrated::Migrated;
pub use rewrap::rewrap;
pub use rewrapped::Rewrapped;
pub use root_key_state::{root_key_state, RootKeyState};
pub use wrapped_root_key::WrappedRootKey;

#[cfg(test)]
pub(in crate::backend) mod test_support;

#[cfg(test)]
mod both_keys_are_detected_explained_and_settled_by_migrate_tests;
#[cfg(test)]
mod crash_before_commit_leaves_the_software_key_and_a_rerun_finishes_tests;
#[cfg(test)]
mod migrate_to_software_reverses_it_tests;
#[cfg(test)]
mod migrate_to_vault_moves_the_key_in_one_step_tests;
#[cfg(test)]
mod migrate_to_vault_refusals_leave_the_software_key_alone_tests;
#[cfg(test)]
mod real_vault_migrate_rewrap_and_back_tests;
#[cfg(test)]
mod retiring_a_version_before_rewrap_is_explained_and_recoverable_tests;
#[cfg(test)]
mod rewrap_follows_a_transit_rotation_tests;
#[cfg(test)]
mod rewrap_refusals_tests;
#[cfg(test)]
mod sealed_data_without_a_root_key_is_not_given_a_new_one_tests;
#[cfg(test)]
mod two_different_keys_are_never_resolved_automatically_tests;
