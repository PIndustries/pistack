// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Vault Transit: wrap and unwrap the root key over HTTP.
//!
//! With this backend the 32-byte data key that seals every secret never
//! touches the store in the clear. `keep` asks Vault's Transit engine to
//! encrypt it (`POST /v1/<mount>/encrypt/<key>`) and stores only the
//! returned `vault:v…` ciphertext in store meta
//! [`META_ROOT_KEY_WRAPPED`](crate::backend::META_ROOT_KEY_WRAPPED). On
//! start it asks Vault to decrypt that ciphertext (`POST
//! /v1/<mount>/decrypt/<key>`) and keeps the data key in memory. After a
//! Transit key rotation, `POST /v1/<mount>/rewrap/<key>` re-encrypts the
//! stored ciphertext under the latest key version without the plaintext
//! leaving Vault (see `pistack keep rewrap`). Payload
//! sealing stays local AES-256-GCM ([`RootKey`](crate::RootKey)); Vault
//! sees one key-sized request per process, not every secret.
//!
//! Calls run on a short-lived worker thread with its own runtime, so this
//! works from synchronous code and from inside an async handler alike.
//! Redirects are not followed, so the token is only ever sent to the
//! configured address.

use base64::{engine::general_purpose::STANDARD as B64, Engine};
use serde_json::{json, Value};
use std::time::Duration;

use crate::error::{KeepError, Result};

/// How long one Transit call may take before it fails.
pub const VAULT_TIMEOUT: Duration = Duration::from_secs(10);

/// Where and how to reach a Vault Transit key. Built by
/// [`KeyBackend::from_lookup`](crate::backend::KeyBackend::from_lookup).
#[derive(Clone)]
pub struct VaultTransit {
    /// Base address without a trailing slash, e.g. `https://vault:8200`.
    pub addr: String,
    pub(crate) token: String,
    /// Transit mount path, default `transit`.
    pub mount: String,
    /// Transit key name.
    pub key: String,
    /// Vault Enterprise namespace (`X-Vault-Namespace`), if any.
    pub namespace: Option<String>,
    /// Extra PEM CA to trust for the Vault listener.
    pub(crate) ca_pem: Option<Vec<u8>>,
    pub timeout: Duration,
}

impl std::fmt::Debug for VaultTransit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VaultTransit")
            .field("addr", &self.addr)
            .field("mount", &self.mount)
            .field("key", &self.key)
            .field("namespace", &self.namespace)
            .field("token", &"..")
            .finish()
    }
}

impl VaultTransit {
    /// Wrap `plaintext`; returns Vault's `vault:v<n>:...` ciphertext.
    pub fn encrypt(&self, plaintext: &[u8]) -> Result<String> {
        let v = self.call("encrypt", json!({ "plaintext": B64.encode(plaintext) }))?;
        let ct = v
            .pointer("/data/ciphertext")
            .and_then(Value::as_str)
            .ok_or_else(|| self.err("encrypt", "response has no data.ciphertext"))?;
        if !ct.starts_with("vault:") {
            return Err(self.err("encrypt", "response ciphertext is not a vault: blob"));
        }
        Ok(ct.to_owned())
    }

    /// Unwrap a ciphertext produced by [`encrypt`](Self::encrypt).
    pub fn decrypt(&self, ciphertext: &str) -> Result<Vec<u8>> {
        let v = self.call("decrypt", json!({ "ciphertext": ciphertext }))?;
        let pt = v
            .pointer("/data/plaintext")
            .and_then(Value::as_str)
            .ok_or_else(|| self.err("decrypt", "response has no data.plaintext"))?;
        B64.decode(pt)
            .map_err(|_| self.err("decrypt", "response plaintext is not base64"))
    }

    /// Re-encrypt `ciphertext` under the latest version of the Transit key.
    /// Vault decrypts and encrypts internally; the plaintext is not returned.
    pub fn rewrap(&self, ciphertext: &str) -> Result<String> {
        let v = self.call("rewrap", json!({ "ciphertext": ciphertext }))?;
        let ct = v
            .pointer("/data/ciphertext")
            .and_then(Value::as_str)
            .ok_or_else(|| self.err("rewrap", "response has no data.ciphertext"))?;
        if !ct.starts_with("vault:") {
            return Err(self.err("rewrap", "response ciphertext is not a vault: blob"));
        }
        Ok(ct.to_owned())
    }

    fn err(&self, op: &str, msg: impl std::fmt::Display) -> KeepError {
        KeepError::Backend(format!(
            "vault transit {op} with key '{}' at {}: {msg}",
            self.key, self.addr
        ))
    }

    fn call(&self, op: &'static str, body: Value) -> Result<Value> {
        let url = format!("{}/v1/{}/{op}/{}", self.addr, self.mount, self.key);
        let this = self.clone();
        let worker = std::thread::Builder::new()
            .name("keep-vault".into())
            .spawn(move || this.call_blocking(op, &url, &body))
            .map_err(|e| self.err(op, format!("cannot start worker: {e}")))?;
        worker.join().map_err(|_| self.err(op, "worker panicked"))?
    }

    fn call_blocking(&self, op: &str, url: &str, body: &Value) -> Result<Value> {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| self.err(op, format!("cannot build runtime: {e}")))?;
        rt.block_on(async {
            let mut builder = reqwest::Client::builder()
                .timeout(self.timeout)
                .redirect(reqwest::redirect::Policy::none());
            if let Some(pem) = &self.ca_pem {
                let ca = reqwest::Certificate::from_pem(pem)
                    .map_err(|e| self.err(op, format!("bad CA certificate: {e}")))?;
                builder = builder.add_root_certificate(ca);
            }
            let client = builder
                .build()
                .map_err(|e| self.err(op, format!("cannot build HTTP client: {e}")))?;
            let mut req = client
                .post(url)
                .header("X-Vault-Token", &self.token)
                .json(body);
            if let Some(ns) = &self.namespace {
                req = req.header("X-Vault-Namespace", ns);
            }
            let resp = req
                .send()
                .await
                .map_err(|e| self.err(op, format!("request failed: {e}")))?;
            let status = resp.status();
            let text = resp
                .text()
                .await
                .map_err(|e| self.err(op, format!("reading response: {e}")))?;
            if !status.is_success() {
                let detail = serde_json::from_str::<Value>(&text)
                    .ok()
                    .and_then(|v| {
                        v.get("errors").and_then(Value::as_array).map(|a| {
                            a.iter()
                                .filter_map(Value::as_str)
                                .collect::<Vec<_>>()
                                .join("; ")
                        })
                    })
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| "no error detail".into());
                return Err(self.err(op, format!("HTTP {status}: {detail}")));
            }
            serde_json::from_str(&text).map_err(|e| self.err(op, format!("bad JSON: {e}")))
        })
    }
}

/// Transit key version `n` of a `vault:v<n>:...` ciphertext.
pub fn ciphertext_version(ciphertext: &str) -> Option<u32> {
    ciphertext
        .strip_prefix("vault:v")?
        .split_once(':')?
        .0
        .parse()
        .ok()
}

/// A Transit look-alike for tests: one named key with versions, token
/// check, real AES-GCM underneath so ciphertext really depends on the
/// server's key. Supports encrypt, decrypt and rewrap, key rotation and
/// `min_decryption_version`, like Transit.
#[cfg(test)]
pub(crate) mod mock {
    use super::*;
    use crate::crypto::{RootKey, Sealed};
    use axum::{
        extract::{Path, State},
        http::{HeaderMap, StatusCode},
        routing::post,
        Json, Router,
    };
    use parking_lot::Mutex;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Arc;

    pub const TOKEN: &str = "s.test-token";
    pub const KEY: &str = "pistack-root";

    pub struct MockVault {
        pub addr: String,
        pub encrypts: AtomicUsize,
        pub decrypts: AtomicUsize,
        pub rewraps: AtomicUsize,
        /// Key versions; version `n` is `keys[n - 1]`.
        keys: Mutex<Vec<RootKey>>,
        min_decryption_version: AtomicUsize,
        /// Decrypt returns the wrong bytes (a broken or hostile Vault).
        pub tamper: AtomicBool,
        /// Rewrap re-encrypts different bytes than it decrypted.
        pub tamper_rewrap: AtomicBool,
        /// Every call fails with 503 (Vault sealed or down).
        pub down: AtomicBool,
    }

    impl MockVault {
        /// Serve on 127.0.0.1 on a background thread for the rest of the
        /// test process.
        pub fn start() -> Arc<MockVault> {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            let addr = format!("http://{}", listener.local_addr().unwrap());
            let state = Arc::new(MockVault {
                addr,
                encrypts: AtomicUsize::new(0),
                decrypts: AtomicUsize::new(0),
                rewraps: AtomicUsize::new(0),
                keys: Mutex::new(vec![RootKey::generate()]),
                min_decryption_version: AtomicUsize::new(1),
                tamper: AtomicBool::new(false),
                tamper_rewrap: AtomicBool::new(false),
                down: AtomicBool::new(false),
            });
            let st = state.clone();
            std::thread::spawn(move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                rt.block_on(async move {
                    let app = Router::new()
                        .route("/v1/transit/{op}/{name}", post(handle))
                        .with_state(st);
                    let l = tokio::net::TcpListener::from_std(listener).unwrap();
                    axum::serve(l, app).await.unwrap();
                });
            });
            state
        }

        /// A backend config pointing at this server.
        pub fn config(&self, token: &str, key: &str) -> VaultTransit {
            VaultTransit {
                addr: self.addr.clone(),
                token: token.into(),
                mount: "transit".into(),
                key: key.into(),
                namespace: None,
                ca_pem: None,
                timeout: Duration::from_secs(5),
            }
        }

        /// `vault write -f transit/keys/<key>/rotate`: returns the new version.
        pub fn rotate(&self) -> usize {
            let mut k = self.keys.lock();
            k.push(RootKey::generate());
            k.len()
        }

        /// `vault write transit/keys/<key>/config min_decryption_version=n`.
        pub fn set_min_decryption_version(&self, n: usize) {
            self.min_decryption_version.store(n, Ordering::SeqCst);
        }

        fn seal(&self, pt: &[u8]) -> String {
            let keys = self.keys.lock();
            let sealed = keys.last().unwrap().seal(pt, b"mock-transit").unwrap();
            let blob = B64.encode(serde_json::to_vec(&sealed).unwrap());
            format!("vault:v{}:{blob}", keys.len())
        }

        fn open(&self, ct: &str) -> std::result::Result<Vec<u8>, &'static str> {
            let n = ciphertext_version(ct).ok_or("invalid ciphertext")? as usize;
            if n < self.min_decryption_version.load(Ordering::SeqCst) {
                return Err("ciphertext or signature version is disallowed by policy (too old)");
            }
            let keys = self.keys.lock();
            let key = keys.get(n.wrapping_sub(1)).ok_or("invalid key version")?;
            let blob = ct.splitn(3, ':').nth(2).ok_or("invalid ciphertext")?;
            B64.decode(blob)
                .ok()
                .and_then(|j| serde_json::from_slice::<Sealed>(&j).ok())
                .and_then(|s| key.open(&s, b"mock-transit").ok())
                .ok_or("invalid ciphertext")
        }
    }

    fn fail(code: StatusCode, msg: &str) -> (StatusCode, Json<Value>) {
        (code, Json(json!({ "errors": [msg] })))
    }

    async fn handle(
        State(st): State<Arc<MockVault>>,
        Path((op, name)): Path<(String, String)>,
        headers: HeaderMap,
        Json(body): Json<Value>,
    ) -> (StatusCode, Json<Value>) {
        if st.down.load(Ordering::SeqCst) {
            return fail(StatusCode::SERVICE_UNAVAILABLE, "Vault is sealed");
        }
        if headers.get("x-vault-token").and_then(|v| v.to_str().ok()) != Some(TOKEN) {
            return fail(StatusCode::FORBIDDEN, "permission denied");
        }
        if name != KEY {
            return fail(StatusCode::BAD_REQUEST, "encryption key not found");
        }
        let ct = || body["ciphertext"].as_str().unwrap_or_default().to_owned();
        match op.as_str() {
            "encrypt" => {
                st.encrypts.fetch_add(1, Ordering::SeqCst);
                let Some(pt) = body["plaintext"].as_str().and_then(|s| B64.decode(s).ok()) else {
                    return fail(StatusCode::BAD_REQUEST, "bad plaintext");
                };
                let c = st.seal(&pt);
                (StatusCode::OK, Json(json!({ "data": { "ciphertext": c } })))
            }
            "decrypt" => {
                st.decrypts.fetch_add(1, Ordering::SeqCst);
                match st.open(&ct()) {
                    Ok(mut pt) => {
                        if st.tamper.load(Ordering::SeqCst) {
                            pt[0] ^= 0xff;
                        }
                        (
                            StatusCode::OK,
                            Json(json!({ "data": { "plaintext": B64.encode(pt) } })),
                        )
                    }
                    Err(m) => fail(StatusCode::BAD_REQUEST, m),
                }
            }
            "rewrap" => {
                st.rewraps.fetch_add(1, Ordering::SeqCst);
                match st.open(&ct()) {
                    Ok(mut pt) => {
                        if st.tamper_rewrap.load(Ordering::SeqCst) {
                            pt[0] ^= 0xff;
                        }
                        let c = st.seal(&pt);
                        let v = ciphertext_version(&c);
                        (
                            StatusCode::OK,
                            Json(json!({ "data": { "ciphertext": c, "key_version": v } })),
                        )
                    }
                    Err(m) => fail(StatusCode::BAD_REQUEST, m),
                }
            }
            _ => fail(StatusCode::NOT_FOUND, "unsupported path"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::mock::{MockVault, KEY, TOKEN};
    use super::*;

    #[test]
    fn transit_round_trip_against_mock() {
        let vault = MockVault::start();
        let cfg = vault.config(TOKEN, KEY);
        let ct = cfg.encrypt(b"0123456789abcdef0123456789abcdef").unwrap();
        assert!(ct.starts_with("vault:v1:"));
        assert!(!ct.contains("0123456789abcdef"));
        assert_eq!(
            cfg.decrypt(&ct).unwrap(),
            b"0123456789abcdef0123456789abcdef"
        );
        // A different Vault (different key material) cannot unwrap it.
        let other = MockVault::start().config(TOKEN, KEY);
        assert!(other.decrypt(&ct).is_err());
    }

    #[test]
    fn transit_errors_are_clear_and_never_leak_the_token() {
        let vault = MockVault::start();
        let e = vault.config("s.wrong", KEY).encrypt(b"k").unwrap_err();
        let msg = e.to_string();
        assert!(matches!(e, KeepError::Backend(_)));
        assert!(msg.contains("403"), "{msg}");
        assert!(msg.contains("permission denied"), "{msg}");
        assert!(!msg.contains("s.wrong"), "{msg}");
        let msg = vault
            .config(TOKEN, "nope")
            .encrypt(b"k")
            .unwrap_err()
            .to_string();
        assert!(msg.contains("encryption key not found"), "{msg}");
        assert!(format!("{:?}", vault.config(TOKEN, KEY)).contains("token: \"..\""));
    }

    #[test]
    fn unreachable_vault_fails() {
        // Bind then drop: nothing listens on this port any more.
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let cfg = VaultTransit {
            addr: format!("http://127.0.0.1:{port}"),
            token: TOKEN.into(),
            mount: "transit".into(),
            key: KEY.into(),
            namespace: None,
            ca_pem: None,
            timeout: Duration::from_secs(2),
        };
        let msg = cfg.encrypt(b"k").unwrap_err().to_string();
        assert!(msg.contains("request failed"), "{msg}");
    }

    #[test]
    fn rewrap_moves_ciphertext_to_the_latest_key_version() {
        let vault = MockVault::start();
        let cfg = vault.config(TOKEN, KEY);
        let ct = cfg.encrypt(b"root").unwrap();
        assert_eq!(ciphertext_version(&ct), Some(1));
        assert_eq!(vault.rotate(), 2);
        let re = cfg.rewrap(&ct).unwrap();
        assert_eq!(ciphertext_version(&re), Some(2));
        assert_eq!(cfg.decrypt(&re).unwrap(), b"root");
        // Raising min_decryption_version locks out the old ciphertext, for
        // decrypt and rewrap alike.
        vault.set_min_decryption_version(2);
        let e = cfg.decrypt(&ct).unwrap_err().to_string();
        assert!(e.contains("too old"), "{e}");
        assert!(cfg.rewrap(&ct).unwrap_err().to_string().contains("too old"));
        assert_eq!(cfg.decrypt(&re).unwrap(), b"root");
        assert_eq!(ciphertext_version("vault:v12:abc"), Some(12));
        assert_eq!(ciphertext_version("vault:x:abc"), None);
        assert_eq!(ciphertext_version("plain"), None);
    }

    #[tokio::test]
    async fn callable_from_inside_a_runtime() {
        let vault = MockVault::start();
        let cfg = vault.config(TOKEN, KEY);
        let ct = cfg.encrypt(b"from-async").unwrap();
        assert_eq!(cfg.decrypt(&ct).unwrap(), b"from-async");
    }
}
