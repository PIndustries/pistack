// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! How one control-plane server proves to another that it is a member.
//!
//! The key is the cluster's peer secret (`gate::token::peer_secret`), which
//! is in every member's store and in every snapshot and nowhere else. A
//! server has it from the moment it holds the cluster's state and needs
//! nothing handed to it. The cluster routes and the object-bytes route both
//! check it.
//!
//! It used to be derived from the token signing secret. It is now its own
//! value, saved once, so the signing keys can rotate without servers
//! losing each other. A cloud from before saves the value it had been
//! deriving, so servers on either build offer and accept the same key.

use pistack_store::Store;
use subtle::ConstantTimeEq;

/// Header a member sends on every call to another server: the peer key.
pub const PEER_HEADER: &str = "x-pistack-cluster";

/// The key members prove themselves with. `None` before the identity
/// module has made its first signing key, which is when the secret is made.
pub fn peer_key(store: &Store) -> Option<String> {
    gate::token::peer_secret(store)
}

/// Whether `offered` is this cluster's peer key. Compared in constant time.
pub fn is_member(store: &Store, offered: &[u8]) -> bool {
    match peer_key(store) {
        Some(key) => bool::from(key.as_bytes().ct_eq(offered)),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_key_of_this_store_is_a_member() {
        let store = Store::in_memory().unwrap();
        assert!(peer_key(&store).is_none());
        assert!(!is_member(&store, b""));
        store
            .put_meta(gate::token::META_JWT_SECRET, &"c2VjcmV0".to_owned())
            .unwrap();
        let key = peer_key(&store).unwrap();
        assert_eq!(key.len(), 64);
        assert!(is_member(&store, key.as_bytes()));
        assert!(!is_member(&store, b"guess"));

        let other = Store::in_memory().unwrap();
        other
            .put_meta(gate::token::META_JWT_SECRET, &"b3RoZXI=".to_owned())
            .unwrap();
        assert!(!is_member(&other, key.as_bytes()));
    }

    #[test]
    fn rotating_the_signing_key_leaves_the_peer_key_alone() {
        let store = Store::in_memory().unwrap();
        store
            .put_meta(gate::token::META_JWT_SECRET, &"c2VjcmV0".to_owned())
            .unwrap();
        // What a server on the build before this one offers and accepts.
        let before = {
            use hmac::Mac;
            let mut mac = hmac::Hmac::<sha2::Sha256>::new_from_slice(b"c2VjcmV0").unwrap();
            mac.update(b"pistack cluster peer v1");
            let d = mac.finalize().into_bytes();
            d.iter().map(|b| format!("{b:02x}")).collect::<String>()
        };
        assert_eq!(peer_key(&store).unwrap(), before);

        gate::token::ensure_saved(&store).unwrap();
        assert_eq!(peer_key(&store).unwrap(), before);
        gate::token::rotate(&store, chrono::Utc::now()).unwrap();
        assert_eq!(peer_key(&store).unwrap(), before);
        // Retiring the old signing key deletes the secret the peer key
        // used to be derived from. The key stays.
        gate::token::retire(&store).unwrap();
        assert!(store
            .get_meta::<String>(gate::token::META_JWT_SECRET)
            .unwrap()
            .is_none());
        assert_eq!(peer_key(&store).unwrap(), before);
        assert!(is_member(&store, before.as_bytes()));
    }
}

/// How servers reach each other. See [`TlsMode`].
pub const CLUSTER_TLS_ENV: &str = "PISTACK_CLUSTER_TLS";

/// Whether cluster traffic may still travel in the clear.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TlsMode {
    /// TLS to a member that has an https address, plain HTTP to one that
    /// has none or whose TLS listener cannot be reached. The default, so a
    /// cluster keeps replicating while its servers are replaced one by one.
    Preferred,
    /// TLS or nothing: this server calls no member in the clear and refuses
    /// the cluster routes on its plain listener.
    Required,
}

/// Read `PISTACK_CLUSTER_TLS`. Unset or empty is `preferred`.
pub fn tls_mode_value(value: Option<&str>) -> std::result::Result<TlsMode, String> {
    match value.map(str::trim) {
        None | Some("") | Some("preferred") => Ok(TlsMode::Preferred),
        Some("required") => Ok(TlsMode::Required),
        Some(other) => Err(format!(
            "{CLUSTER_TLS_ENV} is 'preferred' or 'required', not '{other}'"
        )),
    }
}

/// Where one call to a member goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Route {
    /// Base URL, no trailing slash.
    pub base: String,
    /// Checked against the cloud CA. False sends it as the address says:
    /// plain for `http`, the system's trust for an `https` API address.
    pub tls: bool,
    /// The plain address to use instead when TLS cannot connect.
    fallback: Option<String>,
}

fn trimmed(url: &str) -> String {
    url.trim().trim_end_matches('/').to_owned()
}

/// Pick the address for a member whose record holds `api` and, when it has
/// a TLS listener, `tls_api`. `can_verify` is whether this server holds the
/// cloud CA; `tls_failed` whether TLS to that member failed a moment ago.
pub fn choose(
    api: &str,
    tls_api: Option<&str>,
    mode: TlsMode,
    can_verify: bool,
    tls_failed: bool,
) -> std::result::Result<Route, String> {
    let tls_api = tls_api
        .map(str::trim)
        .filter(|url| url.starts_with("https://"));
    match (mode, tls_api) {
        (TlsMode::Required, None) => Err(format!(
            "{api} has no TLS address and {CLUSTER_TLS_ENV}=required; start that server with --tls-listen"
        )),
        (TlsMode::Required, Some(_)) if !can_verify => Err(format!(
            "this store holds no cloud CA to check a server with and {CLUSTER_TLS_ENV}=required"
        )),
        (TlsMode::Required, Some(url)) => Ok(Route {
            base: trimmed(url),
            tls: true,
            fallback: None,
        }),
        (TlsMode::Preferred, Some(url)) if can_verify && !tls_failed => Ok(Route {
            base: trimmed(url),
            tls: true,
            // An address that is its own TLS address has nothing to fall
            // back to.
            fallback: Some(trimmed(api)).filter(|plain| *plain != trimmed(url)),
        }),
        (TlsMode::Preferred, _) => Ok(Route {
            base: trimmed(api),
            tls: false,
            fallback: None,
        }),
    }
}

/// An error and what caused it, on one line. A failed TLS handshake says
/// why only in its causes.
pub fn error_chain(err: &dyn std::error::Error) -> String {
    let mut text = err.to_string();
    let mut cause = err.source();
    while let Some(inner) = cause {
        text.push_str(": ");
        text.push_str(&inner.to_string());
        cause = inner.source();
    }
    text
}

/// After TLS to a member failed under `preferred`, it is called in the
/// clear for this long before TLS is tried again.
const TLS_RETRY: std::time::Duration = std::time::Duration::from_secs(60);

/// How this server calls the others: which address, and what it trusts.
/// Cheap to clone.
#[derive(Clone)]
pub struct PeerNet {
    inner: std::sync::Arc<Inner>,
}

struct Inner {
    mode: TlsMode,
    /// Where the cloud CA is read from. `None` never uses TLS.
    store: Option<Store>,
    /// The client configuration, once the store had a CA to build it from.
    tls: parking_lot::Mutex<Option<rustls::ClientConfig>>,
    /// TLS addresses that did not connect, and when.
    failed: parking_lot::Mutex<std::collections::HashMap<String, std::time::Instant>>,
}

impl PeerNet {
    /// Calls every member at its API address as recorded. For a process
    /// with no store, and for tests.
    pub fn plain() -> Self {
        Self::build(TlsMode::Preferred, None)
    }

    /// Trust the cloud CA in `store`.
    pub fn new(mode: TlsMode, store: Store) -> Self {
        Self::build(mode, Some(store))
    }

    /// [`Self::new`] with the mode from `PISTACK_CLUSTER_TLS`. A value the
    /// server refused to start with reads as `preferred` here.
    pub fn from_env(store: Store) -> Self {
        let mode = tls_mode_value(std::env::var(CLUSTER_TLS_ENV).ok().as_deref())
            .unwrap_or(TlsMode::Preferred);
        Self::new(mode, store)
    }

    fn build(mode: TlsMode, store: Option<Store>) -> Self {
        Self {
            inner: std::sync::Arc::new(Inner {
                mode,
                store,
                tls: parking_lot::Mutex::new(None),
                failed: parking_lot::Mutex::new(std::collections::HashMap::new()),
            }),
        }
    }

    pub fn mode(&self) -> TlsMode {
        self.inner.mode
    }

    /// The configuration that trusts the cloud CA and nothing else. The CA
    /// of a cloud does not change, so it is read once.
    fn tls_config(&self) -> Option<rustls::ClientConfig> {
        let mut cached = self.inner.tls.lock();
        if cached.is_none() {
            let store = self.inner.store.as_ref()?;
            let bundle = crate::ca::trust_bundle(store).ok()??;
            *cached = crate::tls::client_config(&bundle, None).ok();
        }
        cached.clone()
    }

    /// Where to call the member with these addresses.
    pub fn route(&self, api: &str, tls_api: Option<&str>) -> std::result::Result<Route, String> {
        let failed = tls_api.is_some_and(|url| {
            self.inner
                .failed
                .lock()
                .get(&trimmed(url))
                .is_some_and(|at| at.elapsed() < TLS_RETRY)
        });
        let can_verify = tls_api.is_some() && self.tls_config().is_some();
        choose(api, tls_api, self.inner.mode, can_verify, failed)
    }

    /// TLS to `route` did not connect. Under `preferred` this returns the
    /// plain address to use instead and remembers the failure for a while;
    /// under `required` there is none.
    pub fn fall_back(&self, route: &Route, error: &dyn std::fmt::Display) -> Option<Route> {
        let plain = route.fallback.clone()?;
        let known = self
            .inner
            .failed
            .lock()
            .insert(route.base.clone(), std::time::Instant::now())
            .is_some_and(|at| at.elapsed() < TLS_RETRY * 2);
        if !known {
            tracing::warn!(
                tls = %route.base,
                plain = %plain,
                error = %error,
                "cluster: TLS to this server failed; calling it over plain HTTP for now. {CLUSTER_TLS_ENV}=required forbids that"
            );
        }
        Some(Route {
            base: plain,
            tls: false,
            fallback: None,
        })
    }

    /// Send one request to the member with these addresses. `request`
    /// builds it for the client and base address it is given.
    ///
    /// It goes to the member's TLS listener when it has one, checked
    /// against the cloud CA. When TLS cannot connect, `preferred` sends
    /// the request again to the plain address and `required` fails.
    pub async fn send(
        &self,
        api: &str,
        tls_api: Option<&str>,
        connect_timeout: std::time::Duration,
        request: impl Fn(reqwest::Client, &str) -> reqwest::RequestBuilder,
    ) -> std::result::Result<reqwest::Response, String> {
        let route = self.route(api, tls_api)?;
        let client = self.client(&route, connect_timeout)?;
        match request(client, &route.base).send().await {
            Ok(response) => Ok(response),
            Err(err) if err.is_connect() => {
                let why = error_chain(&err);
                let Some(plain) = self.fall_back(&route, &why) else {
                    return Err(why);
                };
                let client = self.client(&plain, connect_timeout)?;
                let sent = request(client, &plain.base).send().await;
                sent.map_err(|err| error_chain(&err))
            }
            Err(err) => Err(error_chain(&err)),
        }
    }

    /// An HTTP client for `route`.
    pub fn client(
        &self,
        route: &Route,
        connect_timeout: std::time::Duration,
    ) -> std::result::Result<reqwest::Client, String> {
        let builder = reqwest::Client::builder().connect_timeout(connect_timeout);
        let builder = if route.tls {
            let config = self
                .tls_config()
                .ok_or_else(|| "this store holds no cloud CA".to_owned())?;
            builder.use_preconfigured_tls(config)
        } else {
            builder
        };
        builder.build().map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod transport_tests {
    use super::*;

    #[test]
    fn the_setting_is_preferred_unless_it_says_required() {
        assert_eq!(tls_mode_value(None), Ok(TlsMode::Preferred));
        assert_eq!(tls_mode_value(Some("")), Ok(TlsMode::Preferred));
        assert_eq!(tls_mode_value(Some(" preferred ")), Ok(TlsMode::Preferred));
        assert_eq!(tls_mode_value(Some("required")), Ok(TlsMode::Required));
        for bad in ["1", "on", "Required", "off"] {
            assert!(tls_mode_value(Some(bad)).unwrap_err().contains(bad));
        }
    }

    #[test]
    fn a_member_is_called_over_tls_when_it_has_an_address_for_it() {
        let api = "http://10.0.0.12:8443/";
        let tls = Some("https://10.0.0.12:8444");
        let pick =
            |tls_api, mode, can_verify, failed| choose(api, tls_api, mode, can_verify, failed);

        let route = pick(tls, TlsMode::Preferred, true, false).unwrap();
        assert_eq!(route.base, "https://10.0.0.12:8444");
        assert!(route.tls);
        assert_eq!(route.fallback.as_deref(), Some("http://10.0.0.12:8443"));

        // No TLS listener, no CA to check with, or TLS just failed: plain.
        for (tls_api, can_verify, failed) in
            [(None, true, false), (tls, false, false), (tls, true, true)]
        {
            let route = pick(tls_api, TlsMode::Preferred, can_verify, failed).unwrap();
            assert_eq!(route.base, "http://10.0.0.12:8443");
            assert!(!route.tls);
            assert_eq!(route.fallback, None);
        }
        // An agent address that is not https is not a TLS address.
        let route = pick(
            Some("http://10.0.0.12:8444"),
            TlsMode::Preferred,
            true,
            false,
        )
        .unwrap();
        assert!(!route.tls);

        // Required never answers with a plain address.
        let route = pick(tls, TlsMode::Required, true, true).unwrap();
        assert!(route.tls);
        assert_eq!(route.fallback, None);
        assert!(pick(None, TlsMode::Required, true, false)
            .unwrap_err()
            .contains("no TLS address"));
        assert!(pick(tls, TlsMode::Required, false, false)
            .unwrap_err()
            .contains("no cloud CA"));
    }

    #[test]
    fn a_failed_tls_address_is_called_in_the_clear_only_under_preferred() {
        let store = Store::in_memory().unwrap();
        crate::Client::open(&store).unwrap();
        let api = "http://10.0.0.12:8443";
        let tls = Some("https://10.0.0.12:8444");

        let net = PeerNet::new(TlsMode::Preferred, store.clone());
        let route = net.route(api, tls).unwrap();
        assert!(route.tls);
        assert!(net
            .client(&route, std::time::Duration::from_secs(1))
            .is_ok());
        let plain = net.fall_back(&route, &"connection refused").unwrap();
        assert_eq!(plain.base, api);
        assert!(!plain.tls);
        // Remembered: the next call goes straight to the plain address.
        assert_eq!(net.route(api, tls).unwrap(), plain);

        let net = PeerNet::new(TlsMode::Required, store);
        let route = net.route(api, tls).unwrap();
        assert!(route.tls);
        assert_eq!(net.fall_back(&route, &"connection refused"), None);
        assert!(net.route(api, tls).unwrap().tls);
        assert!(net.route(api, None).is_err());

        // A store with no CA, and a process with no store, never use TLS.
        let empty = PeerNet::new(TlsMode::Preferred, Store::in_memory().unwrap());
        assert!(!empty.route(api, tls).unwrap().tls);
        assert!(!PeerNet::plain().route(api, tls).unwrap().tls);
        assert!(PeerNet::new(TlsMode::Required, Store::in_memory().unwrap())
            .route(api, tls)
            .is_err());
    }
}
