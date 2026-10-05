// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Where the root key comes from.
//!
//! [`RootKey`] is the in-memory AES-256-GCM data key that seals every
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

use base64::{engine::general_purpose::STANDARD as B64, Engine};
use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use pistack_store::Store;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::OnceLock;

use crate::crypto::{sha256_hex, RootKey, META_ROOT_KEY};
use crate::error::{KeepError, Result};
use crate::vault::{ciphertext_version, VaultTransit, VAULT_TIMEOUT};

/// Store meta key holding the Vault-wrapped root key ([`WrappedRootKey`]).
pub const META_ROOT_KEY_WRAPPED: &str = "keep/root_key_wrapped";

pub const ENV_BACKEND: &str = "PISTACK_KEEP_BACKEND";
pub const ENV_VAULT_ADDR: &str = "PISTACK_KEEP_VAULT_ADDR";
pub const ENV_VAULT_TOKEN: &str = "PISTACK_KEEP_VAULT_TOKEN";
pub const ENV_VAULT_TOKEN_FILE: &str = "PISTACK_KEEP_VAULT_TOKEN_FILE";
pub const ENV_VAULT_KEY: &str = "PISTACK_KEEP_VAULT_KEY";
pub const ENV_VAULT_MOUNT: &str = "PISTACK_KEEP_VAULT_MOUNT";
pub const ENV_VAULT_NAMESPACE: &str = "PISTACK_KEEP_VAULT_NAMESPACE";
pub const ENV_VAULT_CACERT: &str = "PISTACK_KEEP_VAULT_CACERT";

const VAULT_VARS: [&str; 7] = [
    ENV_VAULT_ADDR,
    ENV_VAULT_TOKEN,
    ENV_VAULT_TOKEN_FILE,
    ENV_VAULT_KEY,
    ENV_VAULT_MOUNT,
    ENV_VAULT_NAMESPACE,
    ENV_VAULT_CACERT,
];

const WRAPPED_BY_VAULT: &str = "vault-transit";

/// The root key as Vault Transit returned it, persisted in store meta.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct WrappedRootKey {
    /// Always `vault-transit` today.
    pub backend: String,
    pub mount: String,
    pub key: String,
    /// `vault:v<n>:...`
    pub ciphertext: String,
    pub created_at: DateTime<Utc>,
    /// Set by [`rewrap`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rewrapped_at: Option<DateTime<Utc>>,
}

/// How the root key is kept at rest.
#[derive(Clone, Debug)]
pub enum KeyBackend {
    /// Plain base64 in store meta. Anyone with the store file has the key.
    Software,
    /// Wrapped by a Vault Transit key; only the ciphertext is stored.
    VaultTransit(VaultTransit),
}

fn backend_err(msg: impl Into<String>) -> KeepError {
    KeepError::Backend(msg.into())
}

impl KeyBackend {
    pub fn name(&self) -> &'static str {
        match self {
            KeyBackend::Software => "software",
            KeyBackend::VaultTransit(_) => "vault",
        }
    }

    /// Read the backend from the process environment.
    pub fn from_env() -> Result<Self> {
        Self::from_lookup(|k| std::env::var(k).ok())
    }

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

    /// Read the root key when one exists. Never creates one, never calls
    /// Vault to wrap.
    pub fn load_existing(&self, store: &Store) -> Result<Option<RootKey>> {
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

    /// Read the root key, creating and persisting one on first start.
    pub fn load_or_create(&self, store: &Store) -> Result<RootKey> {
        if let Some(key) = self.load_existing(store)? {
            return Ok(key);
        }
        refuse_if_sealed_data(store)?;
        let key = RootKey::generate();
        match self {
            KeyBackend::Software => {
                store.put_meta(META_ROOT_KEY, &B64.encode(key.bytes()))?;
                tracing::info!("keep: generated root key (software backend)");
            }
            KeyBackend::VaultTransit(v) => {
                let ciphertext = v.encrypt(key.bytes())?;
                // Prove we can get it back before anything is sealed under it.
                if v.decrypt(&ciphertext)?.as_slice() != key.bytes() {
                    return Err(backend_err(
                        "vault transit returned a different key on unwrap; not storing it",
                    ));
                }
                let w = WrappedRootKey {
                    backend: WRAPPED_BY_VAULT.into(),
                    mount: v.mount.clone(),
                    key: v.key.clone(),
                    ciphertext,
                    created_at: Utc::now(),
                    rewrapped_at: None,
                };
                store.put_meta(META_ROOT_KEY_WRAPPED, &w)?;
                cache().lock().insert(cache_key(v, &w), key.clone());
                tracing::info!(
                    addr = %v.addr,
                    mount = %v.mount,
                    key = %v.key,
                    "keep: generated root key wrapped by Vault Transit"
                );
            }
        }
        Ok(key)
    }
}

/// What a store holds for the root key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RootKeyState {
    /// No key yet: the first server start creates one.
    Empty,
    /// [`META_ROOT_KEY`] only.
    Software,
    /// [`META_ROOT_KEY_WRAPPED`] only.
    Wrapped(WrappedRootKey),
    /// Both: refused at startup until a migrate command settles it.
    Both(WrappedRootKey),
}

pub fn root_key_state(store: &Store) -> Result<RootKeyState> {
    let plain = store.get_meta::<String>(META_ROOT_KEY)?;
    let wrapped = store.get_meta::<WrappedRootKey>(META_ROOT_KEY_WRAPPED)?;
    Ok(match (plain, wrapped) {
        (None, None) => RootKeyState::Empty,
        (Some(_), None) => RootKeyState::Software,
        (None, Some(w)) => RootKeyState::Wrapped(w),
        (Some(_), Some(w)) => RootKeyState::Both(w),
    })
}

impl WrappedRootKey {
    /// Transit key version the ciphertext was made with (`vault:v<n>:`).
    pub fn key_version(&self) -> Option<u32> {
        ciphertext_version(&self.ciphertext)
    }
}

/// Result of [`migrate_to_vault`] or [`migrate_to_software`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Migrated {
    /// Transit mount and key involved.
    pub mount: String,
    pub key: String,
    /// Transit key version of the wrapped key (to vault) or of the one
    /// that was removed (to software).
    pub key_version: Option<u32>,
    /// True when the store already held both keys and this only removed
    /// the stale copy.
    pub resumed: bool,
}

/// Result of [`rewrap`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rewrapped {
    pub from_version: Option<u32>,
    pub to_version: Option<u32>,
    /// False when the ciphertext was already on the latest version; the
    /// store is then left as it was.
    pub changed: bool,
}

/// Move this store's software root key into Vault Transit.
///
/// Wraps [`META_ROOT_KEY`] with `v`, asks Vault to unwrap the result and
/// compares the bytes, and only then writes [`META_ROOT_KEY_WRAPPED`] and
/// deletes [`META_ROOT_KEY`] in one transaction. Any failure before the
/// commit leaves the store as it was. If the store already holds both keys
/// and they are the same key, the plaintext copy is deleted (finishing an
/// interrupted run); two different keys are refused.
pub fn migrate_to_vault(store: &Store, v: &VaultTransit) -> Result<Migrated> {
    let plain = store.get_meta::<String>(META_ROOT_KEY)?;
    let wrapped = store.get_meta::<WrappedRootKey>(META_ROOT_KEY_WRAPPED)?;
    match (plain, wrapped) {
        (None, None) => Err(backend_err(format!(
            "this store has no root key, so there is nothing to migrate; a server started \
             with {ENV_BACKEND}=vault creates a wrapped key on first start"
        ))),
        (None, Some(w)) => Err(backend_err(format!(
            "the root key is already wrapped by Vault Transit (mount '{}', key '{}', v{}); \
             nothing to migrate",
            w.mount,
            w.key,
            w.key_version().unwrap_or(0)
        ))),
        (Some(b64), Some(w)) => {
            check_names(v, &w)?;
            if !both_keys_match(v, &b64, &w)? {
                return Err(both_keys_error(Some(Ok(false))));
            }
            store.delete_meta(META_ROOT_KEY)?;
            tracing::info!(
                "keep: removed the plaintext copy of a root key already wrapped by Vault"
            );
            Ok(Migrated {
                mount: w.mount.clone(),
                key: w.key.clone(),
                key_version: w.key_version(),
                resumed: true,
            })
        }
        (Some(b64), None) => {
            let key = decode_software(&b64)?;
            let ciphertext = v.encrypt(key.bytes())?;
            if v.decrypt(&ciphertext)?.as_slice() != key.bytes() {
                return Err(backend_err(
                    "vault transit unwrapped the new ciphertext to different bytes; \
                     the software key is left in place and nothing was written",
                ));
            }
            let w = WrappedRootKey {
                backend: WRAPPED_BY_VAULT.into(),
                mount: v.mount.clone(),
                key: v.key.clone(),
                ciphertext,
                created_at: Utc::now(),
                rewrapped_at: None,
            };
            store.put_and_delete_meta(META_ROOT_KEY_WRAPPED, &w, META_ROOT_KEY)?;
            cache().lock().insert(cache_key(v, &w), key);
            tracing::info!(
                mount = %w.mount,
                key = %w.key,
                version = w.key_version().unwrap_or(0),
                "keep: moved the root key into Vault Transit"
            );
            Ok(Migrated {
                mount: w.mount.clone(),
                key: w.key.clone(),
                key_version: w.key_version(),
                resumed: false,
            })
        }
    }
}

/// Move a Vault-wrapped root key back into the store in the clear. The
/// reverse of [`migrate_to_vault`]: unwrap, then write [`META_ROOT_KEY`]
/// and delete [`META_ROOT_KEY_WRAPPED`] in one transaction. Afterwards
/// anyone with the store file has the key.
pub fn migrate_to_software(store: &Store, v: &VaultTransit) -> Result<Migrated> {
    let plain = store.get_meta::<String>(META_ROOT_KEY)?;
    let wrapped = store.get_meta::<WrappedRootKey>(META_ROOT_KEY_WRAPPED)?;
    match (plain, wrapped) {
        (_, None) => Err(backend_err(
            "the root key in this store is not wrapped by Vault Transit; nothing to migrate",
        )),
        (Some(b64), Some(w)) => {
            check_names(v, &w)?;
            if !both_keys_match(v, &b64, &w)? {
                return Err(both_keys_error(Some(Ok(false))));
            }
            store.delete_meta(META_ROOT_KEY_WRAPPED)?;
            Ok(Migrated {
                mount: w.mount.clone(),
                key: w.key.clone(),
                key_version: w.key_version(),
                resumed: true,
            })
        }
        (None, Some(w)) => {
            check_names(v, &w)?;
            let key = unwrap_uncached(v, &w)?;
            let b64 = B64.encode(key.bytes());
            if decode_software(&b64)?.bytes() != key.bytes() {
                return Err(backend_err(
                    "root key did not survive encoding; nothing written",
                ));
            }
            store.put_and_delete_meta(META_ROOT_KEY, &b64, META_ROOT_KEY_WRAPPED)?;
            tracing::warn!(
                "keep: root key moved out of Vault; it is now in the store in the clear"
            );
            Ok(Migrated {
                mount: w.mount.clone(),
                key: w.key.clone(),
                key_version: w.key_version(),
                resumed: false,
            })
        }
    }
}

/// Re-encrypt the stored wrapped key under the Transit key's latest
/// version (after a rotation). Vault's rewrap endpoint does the work, so
/// the key is not re-sent; the new ciphertext must unwrap to the same bytes
/// as the old one before it replaces it. Leaves the store alone when the
/// ciphertext is already on the latest version.
pub fn rewrap(store: &Store, v: &VaultTransit) -> Result<Rewrapped> {
    let w = match root_key_state(store)? {
        RootKeyState::Wrapped(w) => w,
        RootKeyState::Both(_) => {
            return Err(backend_err(
                "this store holds both a software and a wrapped root key; finish with \
                 `pistack keep migrate-to-vault` or `pistack keep migrate-to-software` first",
            ))
        }
        RootKeyState::Software | RootKeyState::Empty => {
            return Err(backend_err(
                "the root key in this store is not wrapped by Vault Transit; nothing to rewrap",
            ))
        }
    };
    check_names(v, &w)?;
    let before = unwrap_uncached(v, &w)?;
    let ciphertext = v.rewrap(&w.ciphertext)?;
    let from_version = w.key_version();
    let to_version = ciphertext_version(&ciphertext);
    if to_version.is_none() || to_version < from_version {
        return Err(backend_err(format!(
            "vault transit rewrap returned version {to_version:?}, older than the stored \
             {from_version:?}; nothing written"
        )));
    }
    let next = WrappedRootKey {
        ciphertext,
        rewrapped_at: Some(Utc::now()),
        ..w.clone()
    };
    let after = unwrap_uncached(v, &next)?;
    if after.bytes() != before.bytes() {
        return Err(backend_err(
            "vault transit rewrap produced a ciphertext that unwraps to a different key; \
             nothing written",
        ));
    }
    if to_version == from_version {
        return Ok(Rewrapped {
            from_version,
            to_version,
            changed: false,
        });
    }
    store.put_meta(META_ROOT_KEY_WRAPPED, &next)?;
    cache().lock().insert(cache_key(v, &next), after);
    tracing::info!(
        from = from_version.unwrap_or(0),
        to = to_version.unwrap_or(0),
        "keep: rewrapped the root key under the latest Transit key version"
    );
    Ok(Rewrapped {
        from_version,
        to_version,
        changed: true,
    })
}

fn check_names(v: &VaultTransit, w: &WrappedRootKey) -> Result<()> {
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

/// Do the plaintext and the wrapped copy hold the same key?
fn both_keys_match(v: &VaultTransit, b64: &str, w: &WrappedRootKey) -> Result<bool> {
    check_names(v, w)?;
    let plain = decode_software(b64)?;
    let unwrapped = unwrap_uncached(v, w)?;
    Ok(plain.bytes() == unwrapped.bytes())
}

/// The startup refusal for a store that holds both keys. `matched` is the
/// comparison when Vault settings were available to make one.
fn both_keys_error(matched: Option<Result<bool>>) -> KeepError {
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

/// A store with sealed data but no root key must not get a fresh key: the
/// data would be unreadable and the loss silent.
fn refuse_if_sealed_data(store: &Store) -> Result<()> {
    for (kind, what) in [("sec/", "secrets"), ("ca/", "certificate authorities")] {
        if !store.raw_scan("resources", kind)?.is_empty() {
            return Err(backend_err(format!(
                "this store holds sealed {what} but no root key ({META_ROOT_KEY} or \
                 {META_ROOT_KEY_WRAPPED}); refusing to generate a new key that could not open \
                 them. Restore the root key from a backup"
            )));
        }
    }
    Ok(())
}

/// The process's backend: parsed once from the environment. A bad setting
/// is returned on every call, so nothing silently runs on another backend.
pub fn configured() -> Result<&'static KeyBackend> {
    static CONFIGURED: OnceLock<std::result::Result<KeyBackend, String>> = OnceLock::new();
    CONFIGURED
        .get_or_init(|| KeyBackend::from_env().map_err(|e| e.to_string()))
        .as_ref()
        .map_err(|e| backend_err(e.clone()))
}

fn decode_software(b64: &str) -> Result<RootKey> {
    let raw = B64
        .decode(b64.trim())
        .map_err(|e| KeepError::Crypto(format!("root key is not base64: {e}")))?;
    let bytes: [u8; 32] = raw
        .try_into()
        .map_err(|_| KeepError::Crypto("root key must be 32 bytes".into()))?;
    Ok(RootKey::from_bytes(bytes))
}

fn vault_from(get: &dyn Fn(&str) -> Option<String>) -> Result<VaultTransit> {
    let need =
        |k: &str| get(k).ok_or_else(|| backend_err(format!("{ENV_BACKEND}=vault needs {k}")));
    let addr = need(ENV_VAULT_ADDR)?.trim_end_matches('/').to_owned();
    check_addr(&addr)?;
    let token = match (get(ENV_VAULT_TOKEN), get(ENV_VAULT_TOKEN_FILE)) {
        (Some(_), Some(_)) => {
            return Err(backend_err(format!(
                "set only one of {ENV_VAULT_TOKEN} and {ENV_VAULT_TOKEN_FILE}"
            )))
        }
        (Some(t), None) => t,
        (None, Some(path)) => {
            let t = std::fs::read_to_string(&path)
                .map_err(|e| backend_err(format!("{ENV_VAULT_TOKEN_FILE} {path}: {e}")))?;
            let t = t.trim().to_owned();
            if t.is_empty() {
                return Err(backend_err(format!(
                    "{ENV_VAULT_TOKEN_FILE} {path} is empty"
                )));
            }
            t
        }
        (None, None) => {
            return Err(backend_err(format!(
                "{ENV_BACKEND}=vault needs {ENV_VAULT_TOKEN} or {ENV_VAULT_TOKEN_FILE}"
            )))
        }
    };
    let key = need(ENV_VAULT_KEY)?;
    check_path_part(ENV_VAULT_KEY, &key, false)?;
    let mount = get(ENV_VAULT_MOUNT).unwrap_or_else(|| "transit".into());
    let mount = mount.trim_matches('/').to_owned();
    check_path_part(ENV_VAULT_MOUNT, &mount, true)?;
    let ca_pem = match get(ENV_VAULT_CACERT) {
        Some(path) => Some(
            std::fs::read(&path)
                .map_err(|e| backend_err(format!("{ENV_VAULT_CACERT} {path}: {e}")))?,
        ),
        None => None,
    };
    Ok(VaultTransit {
        addr,
        token,
        mount,
        key,
        namespace: get(ENV_VAULT_NAMESPACE),
        ca_pem,
        timeout: VAULT_TIMEOUT,
    })
}

/// `https://` anywhere; `http://` only to loopback, so a token never
/// crosses the network in the clear.
fn check_addr(addr: &str) -> Result<()> {
    let url = reqwest::Url::parse(addr)
        .map_err(|e| backend_err(format!("{ENV_VAULT_ADDR}={addr:?} is not a URL: {e}")))?;
    let host = url.host_str().unwrap_or_default();
    let loopback = host == "localhost"
        || host
            .trim_matches(|c| c == '[' || c == ']')
            .parse::<IpAddr>()
            .is_ok_and(|ip| ip.is_loopback());
    match url.scheme() {
        "https" => Ok(()),
        "http" if loopback => Ok(()),
        "http" => Err(backend_err(format!(
            "{ENV_VAULT_ADDR}={addr:?}: plain http is only allowed to a loopback address; use https"
        ))),
        s => Err(backend_err(format!(
            "{ENV_VAULT_ADDR}={addr:?}: scheme {s:?} is not http or https"
        ))),
    }
}

fn check_path_part(var: &str, v: &str, allow_slash: bool) -> Result<()> {
    let ok = !v.is_empty()
        && v.split('/')
            .all(|seg| !seg.is_empty() && seg != "." && seg != "..")
        && v.chars().all(|c| {
            c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') || (allow_slash && c == '/')
        });
    if ok {
        Ok(())
    } else {
        Err(backend_err(format!(
            "{var}={v:?} must be [A-Za-z0-9._-]{}",
            if allow_slash {
                " segments joined by '/'"
            } else {
                ""
            }
        )))
    }
}

/// Unwrapped keys by (Vault, key, ciphertext, token fingerprint), so a
/// process asks Vault once rather than on every `Client::open`. A different
/// token is a different entry and must unwrap on its own.
fn cache() -> &'static Mutex<HashMap<String, RootKey>> {
    static CACHE: OnceLock<Mutex<HashMap<String, RootKey>>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

fn cache_key(v: &VaultTransit, w: &WrappedRootKey) -> String {
    format!(
        "{}|{}|{}|{}|{}|{}",
        v.addr,
        v.namespace.as_deref().unwrap_or(""),
        v.mount,
        v.key,
        w.ciphertext,
        sha256_hex(v.token.as_bytes())
    )
}

#[cfg(test)]
pub(crate) fn forget_unwrapped() {
    cache().lock().clear();
}

fn unwrap(v: &VaultTransit, w: &WrappedRootKey) -> Result<RootKey> {
    check_names(v, w)?;
    let ck = cache_key(v, w);
    if let Some(k) = cache().lock().get(&ck) {
        return Ok(k.clone());
    }
    let key = unwrap_uncached(v, w)?;
    cache().lock().insert(ck, key.clone());
    Ok(key)
}

/// Ask Vault, never the cache: the operator commands must prove Vault
/// still unwraps the key.
fn unwrap_uncached(v: &VaultTransit, w: &WrappedRootKey) -> Result<RootKey> {
    let raw = v.decrypt(&w.ciphertext).map_err(|e| too_old_hint(e, w))?;
    let bytes: [u8; 32] = raw
        .try_into()
        .map_err(|_| backend_err("vault transit unwrapped a root key that is not 32 bytes"))?;
    Ok(RootKey::from_bytes(bytes))
}

/// Explain the lock-out after `min_decryption_version` was raised past the
/// version the root key is wrapped with.
fn too_old_hint(e: KeepError, w: &WrappedRootKey) -> KeepError {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vault::mock::{MockVault, KEY, TOKEN};
    use std::collections::BTreeMap;

    fn parse(pairs: &[(&str, &str)]) -> Result<KeyBackend> {
        let env: BTreeMap<String, String> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        KeyBackend::from_lookup(|k| env.get(k).cloned())
    }

    fn err(pairs: &[(&str, &str)]) -> String {
        match parse(pairs) {
            Ok(b) => panic!("expected an error, got {b:?}"),
            Err(e) => {
                assert!(matches!(e, KeepError::Backend(_)), "{e}");
                e.to_string()
            }
        }
    }

    #[test]
    fn selection_defaults_to_software_and_refuses_the_rest() {
        assert!(matches!(parse(&[]).unwrap(), KeyBackend::Software));
        assert!(matches!(
            parse(&[(ENV_BACKEND, "software")]).unwrap(),
            KeyBackend::Software
        ));
        assert!(matches!(
            parse(&[(ENV_BACKEND, "  ")]).unwrap(),
            KeyBackend::Software
        ));
        assert!(err(&[(ENV_BACKEND, "pkcs11")]).contains("parked"));
        assert!(err(&[(ENV_BACKEND, "hsm")]).contains("not a root-key backend"));
        // Vault settings without selecting vault are a mistake, not ignored.
        assert!(err(&[(ENV_VAULT_ADDR, "https://vault:8200")]).contains(ENV_VAULT_ADDR));
    }

    #[test]
    fn vault_settings_are_validated() {
        let base = [
            (ENV_BACKEND, "vault"),
            (ENV_VAULT_ADDR, "https://vault.example:8200/"),
            (ENV_VAULT_TOKEN, "s.abc"),
            (ENV_VAULT_KEY, "pistack-root"),
        ];
        let KeyBackend::VaultTransit(v) = parse(&base).unwrap() else {
            panic!("expected vault");
        };
        assert_eq!(v.addr, "https://vault.example:8200");
        assert_eq!(v.mount, "transit");
        assert_eq!(v.key, "pistack-root");
        assert_eq!(v.token, "s.abc");
        assert!(v.namespace.is_none());

        for missing in [ENV_VAULT_ADDR, ENV_VAULT_KEY] {
            let pairs: Vec<_> = base
                .iter()
                .copied()
                .filter(|(k, _)| *k != missing)
                .collect();
            assert!(err(&pairs).contains(missing));
        }
        let no_token: Vec<_> = base
            .iter()
            .copied()
            .filter(|(k, _)| *k != ENV_VAULT_TOKEN)
            .collect();
        assert!(err(&no_token).contains(ENV_VAULT_TOKEN_FILE));

        let with = |extra: (&'static str, &'static str)| {
            let mut p: Vec<_> = base
                .iter()
                .copied()
                .filter(|(k, _)| *k != extra.0)
                .collect();
            p.push(extra);
            p
        };
        assert!(err(&with((ENV_VAULT_ADDR, "http://vault.example:8200"))).contains("loopback"));
        assert!(parse(&with((ENV_VAULT_ADDR, "http://127.0.0.1:8200"))).is_ok());
        assert!(parse(&with((ENV_VAULT_ADDR, "http://[::1]:8200"))).is_ok());
        assert!(parse(&with((ENV_VAULT_ADDR, "http://localhost:8200"))).is_ok());
        assert!(err(&with((ENV_VAULT_ADDR, "ftp://vault"))).contains("scheme"));
        assert!(err(&with((ENV_VAULT_ADDR, "vault:8200"))).contains(ENV_VAULT_ADDR));
        assert!(err(&with((ENV_VAULT_KEY, "../sys"))).contains(ENV_VAULT_KEY));
        assert!(err(&with((ENV_VAULT_KEY, "a/b"))).contains(ENV_VAULT_KEY));
        assert!(err(&with((ENV_VAULT_MOUNT, "kv/../sys"))).contains(ENV_VAULT_MOUNT));
        let KeyBackend::VaultTransit(v) =
            parse(&with((ENV_VAULT_MOUNT, "/team/transit/"))).unwrap()
        else {
            panic!("expected vault");
        };
        assert_eq!(v.mount, "team/transit");
        assert!(err(&with((ENV_VAULT_TOKEN_FILE, "/nonexistent"))).contains("only one"));
        assert!(err(&with((ENV_VAULT_CACERT, "/nonexistent/ca.pem"))).contains(ENV_VAULT_CACERT));
    }

    #[test]
    fn token_file_is_read_and_must_not_be_empty() {
        let dir = std::env::temp_dir().join(format!("keep-vault-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&dir).unwrap();
        let tok = dir.join("token");
        std::fs::write(&tok, "s.from-file\n").unwrap();
        let empty = dir.join("empty");
        std::fs::write(&empty, "\n").unwrap();
        let pairs = |path: &std::path::Path| {
            parse(&[
                (ENV_BACKEND, "vault"),
                (ENV_VAULT_ADDR, "https://vault:8200"),
                (ENV_VAULT_TOKEN_FILE, path.to_str().unwrap()),
                (ENV_VAULT_KEY, "k"),
            ])
        };
        let KeyBackend::VaultTransit(v) = pairs(&tok).unwrap() else {
            panic!("expected vault");
        };
        assert_eq!(v.token, "s.from-file");
        assert!(pairs(&empty).unwrap_err().to_string().contains("empty"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn vault_backend_wraps_and_unwraps_the_root_key() {
        let vault = MockVault::start();
        let backend = KeyBackend::VaultTransit(vault.config(TOKEN, KEY));
        let store = Store::in_memory().unwrap();
        assert!(backend.load_existing(&store).unwrap().is_none());

        let key = backend.load_or_create(&store).unwrap();
        assert_eq!(vault.encrypts.load(std::sync::atomic::Ordering::SeqCst), 1);
        // Nothing in the clear: no software key, and the wrapped blob does
        // not contain the key bytes.
        assert!(store.get_meta::<String>(META_ROOT_KEY).unwrap().is_none());
        let w: WrappedRootKey = store.get_meta(META_ROOT_KEY_WRAPPED).unwrap().unwrap();
        assert!(w.ciphertext.starts_with("vault:v1:"));
        assert!(!w.ciphertext.contains(&B64.encode(key.bytes())));
        assert_eq!((w.mount.as_str(), w.key.as_str()), ("transit", KEY));

        let sealed = key.seal(b"payload", b"sec-1").unwrap();

        // A fresh process (empty cache) unwraps through Vault and opens it.
        forget_unwrapped();
        let before = vault.decrypts.load(std::sync::atomic::Ordering::SeqCst);
        let again = backend.load_existing(&store).unwrap().unwrap();
        assert!(vault.decrypts.load(std::sync::atomic::Ordering::SeqCst) > before);
        assert_eq!(again.open(&sealed, b"sec-1").unwrap(), b"payload");
        let same = backend.load_or_create(&store).unwrap();
        assert_eq!(same.bytes(), key.bytes());
        assert_eq!(vault.encrypts.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[test]
    fn vault_backend_fails_closed() {
        let vault = MockVault::start();
        let store = Store::in_memory().unwrap();

        // Wrong token on first start: nothing is written.
        let bad = KeyBackend::VaultTransit(vault.config("s.wrong", KEY));
        let e = bad.load_or_create(&store).unwrap_err().to_string();
        assert!(e.contains("permission denied"), "{e}");
        assert!(store.get_meta::<String>(META_ROOT_KEY).unwrap().is_none());
        assert!(store
            .get_meta::<WrappedRootKey>(META_ROOT_KEY_WRAPPED)
            .unwrap()
            .is_none());

        // Create with the right token, then a wrong token cannot unwrap even
        // though this process has the key cached for the right one.
        let good = KeyBackend::VaultTransit(vault.config(TOKEN, KEY));
        good.load_or_create(&store).unwrap();
        let e = bad.load_existing(&store).unwrap_err().to_string();
        assert!(e.contains("permission denied"), "{e}");

        // Another Transit key name is refused before Vault is asked.
        let other = KeyBackend::VaultTransit(vault.config(TOKEN, "other"));
        let e = other.load_existing(&store).unwrap_err().to_string();
        assert!(e.contains("wrapped with Transit mount"), "{e}");

        // The software backend will not touch a Vault-wrapped store.
        let e = KeyBackend::Software
            .load_or_create(&store)
            .unwrap_err()
            .to_string();
        assert!(e.contains("wrapped by Vault Transit"), "{e}");
        assert!(store.get_meta::<String>(META_ROOT_KEY).unwrap().is_none());
    }

    #[test]
    fn vault_backend_refuses_a_software_store() {
        let vault = MockVault::start();
        let store = Store::in_memory().unwrap();
        let soft = KeyBackend::Software.load_or_create(&store).unwrap();
        let backend = KeyBackend::VaultTransit(vault.config(TOKEN, KEY));
        let e = backend.load_or_create(&store).unwrap_err().to_string();
        assert!(e.contains("pistack keep migrate-to-vault"), "{e}");
        assert_eq!(vault.encrypts.load(std::sync::atomic::Ordering::SeqCst), 0);
        // The software key is untouched and still the only one.
        assert!(store
            .get_meta::<WrappedRootKey>(META_ROOT_KEY_WRAPPED)
            .unwrap()
            .is_none());
        let still = KeyBackend::Software.load_existing(&store).unwrap().unwrap();
        assert_eq!(still.bytes(), soft.bytes());
    }

    #[test]
    fn both_keys_present_is_an_error() {
        let store = Store::in_memory().unwrap();
        KeyBackend::Software.load_or_create(&store).unwrap();
        store
            .put_meta(
                META_ROOT_KEY_WRAPPED,
                &WrappedRootKey {
                    backend: WRAPPED_BY_VAULT.into(),
                    mount: "transit".into(),
                    key: KEY.into(),
                    ciphertext: "vault:v1:x".into(),
                    created_at: Utc::now(),
                    rewrapped_at: None,
                },
            )
            .unwrap();
        let e = KeyBackend::Software
            .load_existing(&store)
            .unwrap_err()
            .to_string();
        assert!(e.contains("both"), "{e}");
    }

    // ---- migration and rewrap ----------------------------------------------

    use crate::client::Client;
    use crate::types::{Secret, SecretSpec};
    use pistack_store::{Backend, LogEntry, Op, RedbBackend};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    /// A store whose next transaction can be made to "crash": the batch is
    /// dropped before commit, as a power cut would drop it.
    struct Crashy {
        inner: RedbBackend,
        crash: AtomicBool,
    }

    impl Backend for Crashy {
        fn get(&self, t: &str, k: &str) -> pistack_store::Result<Option<Vec<u8>>> {
            self.inner.get(t, k)
        }
        fn put(&self, t: &str, k: &str, v: &[u8]) -> pistack_store::Result<()> {
            self.batch(vec![Op::Put {
                table: t.into(),
                key: k.into(),
                value: v.to_vec(),
            }])
        }
        fn delete(&self, t: &str, k: &str) -> pistack_store::Result<bool> {
            let had = self.inner.get(t, k)?.is_some();
            self.batch(vec![Op::Delete {
                table: t.into(),
                key: k.into(),
            }])?;
            Ok(had)
        }
        fn scan(&self, t: &str, p: &str) -> pistack_store::Result<Vec<(String, Vec<u8>)>> {
            self.inner.scan(t, p)
        }
        fn batch(&self, ops: Vec<Op>) -> pistack_store::Result<()> {
            if self.crash.load(Ordering::SeqCst) {
                return Err(pistack_store::StoreError::Storage(
                    "simulated crash before commit".into(),
                ));
            }
            self.inner.batch(ops)
        }
        fn set_record_log(&self, on: bool) {
            self.inner.set_record_log(on)
        }
        fn set_log_term(&self, term: u64) {
            self.inner.set_log_term(term)
        }
        fn read_log(&self, after: u64, limit: usize) -> pistack_store::Result<Vec<LogEntry>> {
            self.inner.read_log(after, limit)
        }
        fn record_log(&self) -> bool {
            self.inner.record_log()
        }
    }

    fn crashy_store() -> (Store, Arc<Crashy>) {
        let b = Arc::new(Crashy {
            inner: RedbBackend::in_memory().unwrap(),
            crash: AtomicBool::new(false),
        });
        let (tx, _) = tokio::sync::broadcast::channel(16);
        (Store::new(b.clone(), tx), b)
    }

    fn state(store: &Store) -> RootKeyState {
        root_key_state(store).unwrap()
    }

    /// A software store with one secret and the CA, as a running cloud has.
    fn software_cloud(store: &Store) -> (RootKey, Id, String) {
        let c = Client::open_with(store, &KeyBackend::Software).unwrap();
        let s = c
            .create_secret(Secret::new("db", None, SecretSpec::default()), b"pw")
            .unwrap();
        let key = KeyBackend::Software.load_existing(store).unwrap().unwrap();
        (key, s.meta.id, c.ca_bundle_pem())
    }

    use pistack_types::Id;

    fn reopen_and_check(store: &Store, backend: &KeyBackend, secret: &Id, ca_pem: &str) {
        forget_unwrapped();
        let c = Client::open_with(store, backend).unwrap();
        assert_eq!(c.get_secret_payload(secret).unwrap(), b"pw");
        assert_eq!(c.ca_bundle_pem(), ca_pem);
    }

    #[test]
    fn migrate_to_vault_moves_the_key_in_one_step() {
        let vault = MockVault::start();
        let v = vault.config(TOKEN, KEY);
        let store = Store::in_memory().unwrap();
        let (key, secret, ca) = software_cloud(&store);

        let m = migrate_to_vault(&store, &v).unwrap();
        assert_eq!(
            m,
            Migrated {
                mount: "transit".into(),
                key: KEY.into(),
                key_version: Some(1),
                resumed: false
            }
        );
        let RootKeyState::Wrapped(w) = state(&store) else {
            panic!("expected only the wrapped key");
        };
        assert!(!w.ciphertext.contains(&B64.encode(key.bytes())));
        // Same key, now from Vault: existing secrets and the CA still open.
        let vb = KeyBackend::VaultTransit(v.clone());
        forget_unwrapped();
        assert_eq!(
            vb.load_existing(&store).unwrap().unwrap().bytes(),
            key.bytes()
        );
        reopen_and_check(&store, &vb, &secret, &ca);
        // The software backend now refuses, and a second run has nothing to do.
        assert!(KeyBackend::Software.load_existing(&store).is_err());
        let e = migrate_to_vault(&store, &v).unwrap_err().to_string();
        assert!(e.contains("already wrapped"), "{e}");
    }

    #[test]
    fn migrate_to_vault_refusals_leave_the_software_key_alone() {
        let vault = MockVault::start();
        let store = Store::in_memory().unwrap();
        // Nothing to migrate in an empty store.
        let e = migrate_to_vault(&store, &vault.config(TOKEN, KEY))
            .unwrap_err()
            .to_string();
        assert!(e.contains("no root key"), "{e}");
        assert_eq!(state(&store), RootKeyState::Empty);

        let (key, secret, ca) = software_cloud(&store);
        let unchanged = |store: &Store| {
            assert_eq!(state(store), RootKeyState::Software);
            reopen_and_check(store, &KeyBackend::Software, &secret, &ca);
            let k = KeyBackend::Software.load_existing(store).unwrap().unwrap();
            assert_eq!(k.bytes(), key.bytes());
        };
        // Vault refuses the token.
        let e = migrate_to_vault(&store, &vault.config("s.wrong", KEY))
            .unwrap_err()
            .to_string();
        assert!(e.contains("permission denied"), "{e}");
        unchanged(&store);
        // Vault is down.
        vault.down.store(true, Ordering::SeqCst);
        let e = migrate_to_vault(&store, &vault.config(TOKEN, KEY))
            .unwrap_err()
            .to_string();
        assert!(e.contains("sealed"), "{e}");
        vault.down.store(false, Ordering::SeqCst);
        unchanged(&store);
        // Vault unwraps to something else: the round-trip check catches it.
        vault.tamper.store(true, Ordering::SeqCst);
        let e = migrate_to_vault(&store, &vault.config(TOKEN, KEY))
            .unwrap_err()
            .to_string();
        assert!(e.contains("different bytes"), "{e}");
        vault.tamper.store(false, Ordering::SeqCst);
        unchanged(&store);
        // Startup with vault settings explains what to run.
        let e = KeyBackend::VaultTransit(vault.config(TOKEN, KEY))
            .load_or_create(&store)
            .unwrap_err()
            .to_string();
        assert!(e.contains("pistack keep migrate-to-vault"), "{e}");
        unchanged(&store);
    }

    #[test]
    fn crash_before_commit_leaves_the_software_key_and_a_rerun_finishes() {
        let vault = MockVault::start();
        let v = vault.config(TOKEN, KEY);
        let (store, crashy) = crashy_store();
        let (key, secret, ca) = software_cloud(&store);

        crashy.crash.store(true, Ordering::SeqCst);
        let e = migrate_to_vault(&store, &v).unwrap_err().to_string();
        assert!(e.contains("simulated crash"), "{e}");
        crashy.crash.store(false, Ordering::SeqCst);
        // Vault was asked (wrap + verify), but the store never saw a write:
        // still exactly the software key, never neither and never both.
        assert_eq!(vault.encrypts.load(Ordering::SeqCst), 1);
        assert_eq!(state(&store), RootKeyState::Software);
        reopen_and_check(&store, &KeyBackend::Software, &secret, &ca);

        // Running it again completes.
        migrate_to_vault(&store, &v).unwrap();
        assert!(matches!(state(&store), RootKeyState::Wrapped(_)));
        let vb = KeyBackend::VaultTransit(v);
        reopen_and_check(&store, &vb, &secret, &ca);
        forget_unwrapped();
        assert_eq!(
            vb.load_existing(&store).unwrap().unwrap().bytes(),
            key.bytes()
        );
    }

    #[test]
    fn both_keys_are_detected_explained_and_settled_by_migrate() {
        let vault = MockVault::start();
        let v = vault.config(TOKEN, KEY);
        let vb = KeyBackend::VaultTransit(v.clone());
        let store = Store::in_memory().unwrap();
        let (key, secret, ca) = software_cloud(&store);
        // What a migration that wrote the wrapped key but died before
        // deleting the plaintext one would leave.
        let w = WrappedRootKey {
            backend: WRAPPED_BY_VAULT.into(),
            mount: "transit".into(),
            key: KEY.into(),
            ciphertext: v.encrypt(key.bytes()).unwrap(),
            created_at: Utc::now(),
            rewrapped_at: None,
        };
        store.put_meta(META_ROOT_KEY_WRAPPED, &w).unwrap();
        assert!(matches!(state(&store), RootKeyState::Both(_)));

        // Startup refuses with either backend and says what to do.
        forget_unwrapped();
        let e = vb.load_or_create(&store).unwrap_err().to_string();
        assert!(e.contains("same root key twice"), "{e}");
        assert!(e.contains("pistack keep migrate-to-vault"), "{e}");
        let e = KeyBackend::Software
            .load_or_create(&store)
            .unwrap_err()
            .to_string();
        assert!(e.contains("refusing to guess"), "{e}");
        assert!(e.contains("migrate-to-software"), "{e}");
        // Rewrap will not touch it either.
        assert!(rewrap(&store, &v)
            .unwrap_err()
            .to_string()
            .contains("first"));

        // migrate-to-vault finishes the job: only the wrapped key remains.
        let m = migrate_to_vault(&store, &v).unwrap();
        assert!(m.resumed);
        assert_eq!(state(&store), RootKeyState::Wrapped(w.clone()));
        reopen_and_check(&store, &vb, &secret, &ca);

        // The same state settled the other way keeps the plaintext key.
        let store2 = Store::in_memory().unwrap();
        let (key2, secret2, ca2) = software_cloud(&store2);
        let w2 = WrappedRootKey {
            ciphertext: v.encrypt(key2.bytes()).unwrap(),
            ..w.clone()
        };
        store2.put_meta(META_ROOT_KEY_WRAPPED, &w2).unwrap();
        assert!(migrate_to_software(&store2, &v).unwrap().resumed);
        assert_eq!(state(&store2), RootKeyState::Software);
        reopen_and_check(&store2, &KeyBackend::Software, &secret2, &ca2);
    }

    #[test]
    fn two_different_keys_are_never_resolved_automatically() {
        let vault = MockVault::start();
        let v = vault.config(TOKEN, KEY);
        let store = Store::in_memory().unwrap();
        software_cloud(&store);
        let other = RootKey::generate();
        store
            .put_meta(
                META_ROOT_KEY_WRAPPED,
                &WrappedRootKey {
                    backend: WRAPPED_BY_VAULT.into(),
                    mount: "transit".into(),
                    key: KEY.into(),
                    ciphertext: v.encrypt(other.bytes()).unwrap(),
                    created_at: Utc::now(),
                    rewrapped_at: None,
                },
            )
            .unwrap();
        let before = state(&store);
        forget_unwrapped();
        let e = KeyBackend::VaultTransit(v.clone())
            .load_existing(&store)
            .unwrap_err()
            .to_string();
        assert!(e.contains("two different root keys"), "{e}");
        for r in [
            migrate_to_vault(&store, &v),
            migrate_to_software(&store, &v),
        ] {
            let e = r.unwrap_err().to_string();
            assert!(e.contains("two different root keys"), "{e}");
        }
        assert_eq!(state(&store), before);
        assert!(store.get_meta::<String>(META_ROOT_KEY).unwrap().is_some());
    }

    #[test]
    fn migrate_to_software_reverses_it() {
        let vault = MockVault::start();
        let v = vault.config(TOKEN, KEY);
        let store = Store::in_memory().unwrap();
        let (key, secret, ca) = software_cloud(&store);
        migrate_to_vault(&store, &v).unwrap();

        // A wrong token changes nothing.
        let e = migrate_to_software(&store, &vault.config("s.wrong", KEY))
            .unwrap_err()
            .to_string();
        assert!(e.contains("permission denied"), "{e}");
        assert!(matches!(state(&store), RootKeyState::Wrapped(_)));

        let m = migrate_to_software(&store, &v).unwrap();
        assert!(!m.resumed);
        assert_eq!(state(&store), RootKeyState::Software);
        let k = KeyBackend::Software.load_existing(&store).unwrap().unwrap();
        assert_eq!(k.bytes(), key.bytes());
        reopen_and_check(&store, &KeyBackend::Software, &secret, &ca);
        let e = migrate_to_software(&store, &v).unwrap_err().to_string();
        assert!(e.contains("not wrapped"), "{e}");
    }

    #[test]
    fn rewrap_follows_a_transit_rotation() {
        let vault = MockVault::start();
        let v = vault.config(TOKEN, KEY);
        let vb = KeyBackend::VaultTransit(v.clone());
        let store = Store::in_memory().unwrap();
        let (key, secret, ca) = software_cloud(&store);
        migrate_to_vault(&store, &v).unwrap();
        let RootKeyState::Wrapped(w1) = state(&store) else {
            panic!()
        };

        // Already on the latest version: reported, nothing written.
        let r = rewrap(&store, &v).unwrap();
        assert_eq!(
            r,
            Rewrapped {
                from_version: Some(1),
                to_version: Some(1),
                changed: false
            }
        );
        assert_eq!(state(&store), RootKeyState::Wrapped(w1.clone()));

        vault.rotate();
        let r = rewrap(&store, &v).unwrap();
        assert_eq!(
            (r.from_version, r.to_version, r.changed),
            (Some(1), Some(2), true)
        );
        let RootKeyState::Wrapped(w2) = state(&store) else {
            panic!()
        };
        assert!(w2.ciphertext.starts_with("vault:v2:"));
        assert_eq!(w2.created_at, w1.created_at);
        assert!(w2.rewrapped_at.is_some());

        // Now the old version can be retired without locking keep out.
        vault.set_min_decryption_version(2);
        forget_unwrapped();
        assert_eq!(
            vb.load_existing(&store).unwrap().unwrap().bytes(),
            key.bytes()
        );
        reopen_and_check(&store, &vb, &secret, &ca);
    }

    #[test]
    fn retiring_a_version_before_rewrap_is_explained_and_recoverable() {
        let vault = MockVault::start();
        let v = vault.config(TOKEN, KEY);
        let vb = KeyBackend::VaultTransit(v.clone());
        let store = Store::in_memory().unwrap();
        let (_, secret, ca) = software_cloud(&store);
        migrate_to_vault(&store, &v).unwrap();
        let before = state(&store);

        vault.rotate();
        vault.set_min_decryption_version(2);
        forget_unwrapped();
        let e = vb.load_existing(&store).unwrap_err().to_string();
        assert!(e.contains("too old"), "{e}");
        assert!(e.contains("Lower min_decryption_version to 1"), "{e}");
        assert!(e.contains("pistack keep rewrap"), "{e}");
        // Rewrap cannot help while the version is retired, and writes nothing.
        let e = rewrap(&store, &v).unwrap_err().to_string();
        assert!(e.contains("min_decryption_version"), "{e}");
        assert_eq!(state(&store), before);

        // The documented way out.
        vault.set_min_decryption_version(1);
        assert!(rewrap(&store, &v).unwrap().changed);
        vault.set_min_decryption_version(2);
        reopen_and_check(&store, &vb, &secret, &ca);
    }

    #[test]
    fn rewrap_refusals() {
        let vault = MockVault::start();
        let v = vault.config(TOKEN, KEY);
        let store = Store::in_memory().unwrap();
        assert!(rewrap(&store, &v)
            .unwrap_err()
            .to_string()
            .contains("not wrapped"));
        software_cloud(&store);
        assert!(rewrap(&store, &v)
            .unwrap_err()
            .to_string()
            .contains("not wrapped"));
        migrate_to_vault(&store, &v).unwrap();
        let before = state(&store);
        let e = rewrap(&store, &vault.config(TOKEN, "other"))
            .unwrap_err()
            .to_string();
        assert!(e.contains("wrapped with Transit mount"), "{e}");
        vault.rotate();
        vault.tamper_rewrap.store(true, Ordering::SeqCst);
        let e = rewrap(&store, &v).unwrap_err().to_string();
        assert!(e.contains("different key"), "{e}");
        vault.tamper_rewrap.store(false, Ordering::SeqCst);
        let e = rewrap(&store, &vault.config("s.wrong", KEY))
            .unwrap_err()
            .to_string();
        assert!(e.contains("permission denied"), "{e}");
        assert_eq!(state(&store), before);
    }

    #[test]
    fn sealed_data_without_a_root_key_is_not_given_a_new_one() {
        let store = Store::in_memory().unwrap();
        software_cloud(&store);
        store.delete_meta(META_ROOT_KEY).unwrap();
        let vault = MockVault::start();
        for b in [
            KeyBackend::Software,
            KeyBackend::VaultTransit(vault.config(TOKEN, KEY)),
        ] {
            let e = b.load_or_create(&store).unwrap_err().to_string();
            assert!(e.contains("sealed"), "{e}");
        }
        assert_eq!(state(&store), RootKeyState::Empty);
        assert_eq!(vault.encrypts.load(Ordering::SeqCst), 0);
    }

    // ---- against a real Vault ----------------------------------------------

    /// Admin call for the real-Vault test (mounts, keys, rotation).
    fn vault_admin(addr: &str, token: &str, method: &str, path: &str, body: serde_json::Value) {
        let (url, token, method) = (
            format!("{addr}/v1/{path}"),
            token.to_owned(),
            method.to_owned(),
        );
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            rt.block_on(async move {
                let c = reqwest::Client::new();
                let req = match method.as_str() {
                    "DELETE" => c.delete(&url),
                    _ => c.post(&url).json(&body),
                };
                let resp = req.header("X-Vault-Token", token).send().await.unwrap();
                let status = resp.status();
                let text = resp.text().await.unwrap_or_default();
                assert!(status.is_success(), "{method} {url}: {status} {text}");
            })
        })
        .join()
        .unwrap();
    }

    /// Migration, rewrap and the lock-out path against a real Vault, e.g.
    /// `vault server -dev`. Ignored by default and not run in CI:
    ///
    /// ```text
    /// PISTACK_TEST_VAULT_ADDR=http://127.0.0.1:8200 PISTACK_TEST_VAULT_TOKEN=root \
    ///   cargo test -p keep --lib real_vault -- --ignored
    /// ```
    ///
    /// It mounts a fresh Transit engine at a random path and removes it after.
    #[test]
    #[ignore = "needs a real Vault: set PISTACK_TEST_VAULT_ADDR and PISTACK_TEST_VAULT_TOKEN"]
    fn real_vault_migrate_rewrap_and_back() {
        let addr = std::env::var("PISTACK_TEST_VAULT_ADDR").expect("PISTACK_TEST_VAULT_ADDR");
        let token = std::env::var("PISTACK_TEST_VAULT_TOKEN").expect("PISTACK_TEST_VAULT_TOKEN");
        let mount = format!(
            "pistack-it-{}",
            &uuid::Uuid::now_v7().simple().to_string()[24..]
        );
        let key = "pistack-root";
        vault_admin(
            &addr,
            &token,
            "POST",
            &format!("sys/mounts/{mount}"),
            serde_json::json!({"type": "transit"}),
        );
        let result = std::panic::catch_unwind(|| {
            vault_admin(
                &addr,
                &token,
                "POST",
                &format!("{mount}/keys/{key}"),
                serde_json::json!({}),
            );
            let backend = parse(&[
                (ENV_BACKEND, "vault"),
                (ENV_VAULT_ADDR, &addr),
                (ENV_VAULT_TOKEN, &token),
                (ENV_VAULT_KEY, key),
                (ENV_VAULT_MOUNT, &mount),
            ])
            .unwrap();
            let KeyBackend::VaultTransit(v) = backend.clone() else {
                panic!()
            };
            let store = Store::in_memory().unwrap();
            let (soft, secret, ca) = software_cloud(&store);

            // Software -> Vault.
            let m = migrate_to_vault(&store, &v).unwrap();
            assert_eq!(m.key_version, Some(1));
            assert!(matches!(state(&store), RootKeyState::Wrapped(_)));
            reopen_and_check(&store, &backend, &secret, &ca);

            // Rotate, rewrap, retire v1: still opens.
            vault_admin(
                &addr,
                &token,
                "POST",
                &format!("{mount}/keys/{key}/rotate"),
                serde_json::json!({}),
            );
            let r = rewrap(&store, &v).unwrap();
            assert_eq!(
                (r.from_version, r.to_version, r.changed),
                (Some(1), Some(2), true)
            );
            assert!(!rewrap(&store, &v).unwrap().changed);
            vault_admin(
                &addr,
                &token,
                "POST",
                &format!("{mount}/keys/{key}/config"),
                serde_json::json!({"min_decryption_version": 2}),
            );
            reopen_and_check(&store, &backend, &secret, &ca);

            // Retire v2 before rewrapping: refused with the way out.
            vault_admin(
                &addr,
                &token,
                "POST",
                &format!("{mount}/keys/{key}/rotate"),
                serde_json::json!({}),
            );
            vault_admin(
                &addr,
                &token,
                "POST",
                &format!("{mount}/keys/{key}/config"),
                serde_json::json!({"min_decryption_version": 3}),
            );
            forget_unwrapped();
            let e = backend.load_existing(&store).unwrap_err().to_string();
            assert!(e.contains("Lower min_decryption_version to 2"), "{e}");
            vault_admin(
                &addr,
                &token,
                "POST",
                &format!("{mount}/keys/{key}/config"),
                serde_json::json!({"min_decryption_version": 2}),
            );
            assert!(rewrap(&store, &v).unwrap().changed);

            // Vault -> software gives back the original key.
            migrate_to_software(&store, &v).unwrap();
            let k = KeyBackend::Software.load_existing(&store).unwrap().unwrap();
            assert_eq!(k.bytes(), soft.bytes());
            reopen_and_check(&store, &KeyBackend::Software, &secret, &ca);
        });
        vault_admin(
            &addr,
            &token,
            "DELETE",
            &format!("sys/mounts/{mount}"),
            serde_json::json!({}),
        );
        if let Err(p) = result {
            std::panic::resume_unwind(p);
        }
    }
}
