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
//! | `vault` | partial (see below) | wrapped by Vault Transit in [`META_ROOT_KEY_WRAPPED`] |
//! | `pkcs11` | parked | refused at startup: not implemented |
//!
//! The Vault backend wraps and unwraps the key for real (see `vault.rs`)
//! and is tested against a mock Transit server. Not done yet: migrating an
//! existing software key into Vault, re-wrapping after a Transit key
//! rotation, and a run against a real Vault in the lab.
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
use crate::vault::{VaultTransit, VAULT_TIMEOUT};

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
            (_, Some(_), Some(_)) => Err(backend_err(format!(
                "store holds both a software root key ({META_ROOT_KEY}) and a wrapped one \
                 ({META_ROOT_KEY_WRAPPED}); refusing to guess which is live"
            ))),
            (_, None, None) => Ok(None),
            (KeyBackend::Software, Some(b64), None) => decode_software(&b64).map(Some),
            (KeyBackend::Software, None, Some(w)) => Err(backend_err(format!(
                "the root key in this store is wrapped by Vault Transit (mount '{}', key '{}'); \
                 set {ENV_BACKEND}=vault with matching settings",
                w.mount, w.key
            ))),
            (KeyBackend::VaultTransit(_), Some(_), None) => Err(backend_err(format!(
                "{ENV_BACKEND}=vault but this store holds a software root key in \
                 {META_ROOT_KEY}; moving an existing key into Vault is not implemented, \
                 so it is neither used nor replaced"
            ))),
            (KeyBackend::VaultTransit(v), None, Some(w)) => unwrap(v, &w).map(Some),
        }
    }

    /// Read the root key, creating and persisting one on first start.
    pub fn load_or_create(&self, store: &Store) -> Result<RootKey> {
        if let Some(key) = self.load_existing(store)? {
            return Ok(key);
        }
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
    let ck = cache_key(v, w);
    if let Some(k) = cache().lock().get(&ck) {
        return Ok(k.clone());
    }
    let raw = v.decrypt(&w.ciphertext)?;
    let bytes: [u8; 32] = raw
        .try_into()
        .map_err(|_| backend_err("vault transit unwrapped a root key that is not 32 bytes"))?;
    let key = RootKey::from_bytes(bytes);
    cache().lock().insert(ck, key.clone());
    Ok(key)
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
        assert!(e.contains("not implemented"), "{e}");
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
                },
            )
            .unwrap();
        let e = KeyBackend::Software
            .load_existing(&store)
            .unwrap_err()
            .to_string();
        assert!(e.contains("both"), "{e}");
    }
}
