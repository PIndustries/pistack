// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Finding a provider's signing keys: the OpenID discovery document, then its
//! JWKS, over HTTPS, cached per provider and refetched when a token names a
//! key the cache does not hold.
//!
//! The cache holds the JWKS text for ten minutes per provider, not parsed
//! keys: `jsonwebtoken::DecodingKey` is not `Clone`, so a cache hit re-parses
//! the stored text into a fresh key set. The same rule that decides which
//! tokens verify decides which keys are kept: only `RS256` and `ES256`. An
//! `oct` key (a shared secret) or any other type is dropped here, so an
//! HMAC-capable key can never reach the verifier.

use crate::oidc_error::Reason;
use crate::types::IdentityProvider;
use base64::Engine;
use jsonwebtoken::{Algorithm, DecodingKey};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// How long a provider's key set is reused before it is fetched again.
const TTL: Duration = Duration::from_secs(600);
/// Seconds allowed for the whole discovery-and-JWKS exchange.
const FETCH_TIMEOUT: Duration = Duration::from_secs(5);
const WELL_KNOWN: &str = "/.well-known/openid-configuration";

/// A verification key and the algorithm its `kty` names.
pub struct ProviderKey {
    pub kid: Option<String>,
    pub alg: Algorithm,
    pub key: DecodingKey,
}

/// One provider's keys, with the moment they were fetched.
pub struct ProviderKeys {
    keys: Vec<ProviderKey>,
    /// When the set was fetched, kept for callers that want the age; freshness
    /// is decided on the cache entry, not here.
    #[allow(dead_code)]
    fetched_at: Instant,
}

impl ProviderKeys {
    /// The key with `kid`, or the only key when the token names none and the
    /// set holds exactly one.
    pub fn find(&self, kid: Option<&str>) -> Option<&ProviderKey> {
        match kid {
            Some(kid) => self.keys.iter().find(|k| k.kid.as_deref() == Some(kid)),
            None if self.keys.len() == 1 => self.keys.first(),
            None => None,
        }
    }

    /// Whether any key carries this `kid`. Drives the "not in the cache"
    /// refetch: a token naming a key we do not hold means the provider likely
    /// rotated its set.
    pub fn has_kid(&self, kid: &str) -> bool {
        self.keys.iter().any(|k| k.kid.as_deref() == Some(kid))
    }
}

/// One provider's cached entry: the JWKS text and when it arrived.
#[derive(Clone)]
struct Cached {
    jwks: String,
    fetched_at: Instant,
}

impl Cached {
    /// Whether the entry is young enough to reuse without a fetch.
    fn fresh(&self) -> bool {
        self.fetched_at.elapsed() < TTL
    }
}

/// Every provider's cached keys, shared by the threads of this server. A
/// poisoned lock is treated as empty rather than taking sign-ins down.
fn cache() -> &'static Mutex<HashMap<String, Cached>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Cached>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn lock_cache() -> std::sync::MutexGuard<'static, HashMap<String, Cached>> {
    cache().lock().unwrap_or_else(|e| e.into_inner())
}

/// The keys for `provider`: the cache if it holds a fresh set, otherwise a
/// fetch. A provider whose issuer is not a URL it may fetch is a hard
/// misconfiguration and never reaches the network.
pub async fn keys_for(provider: &IdentityProvider) -> Result<ProviderKeys, Reason> {
    let cache_key = provider.meta.id.to_string();
    // Copy the entry out under the lock, then drop the guard: an `await` must
    // not hold it.
    let hit = {
        let guard = lock_cache();
        guard.get(&cache_key).filter(|c| c.fresh()).cloned()
    };
    let cached = match hit {
        Some(cached) => cached,
        None => fetch(provider).await?,
    };
    Ok(parse_jwks(&cached.jwks, cached.fetched_at))
}

/// The keys again, ignoring the cache. Called once when a token names a `kid`
/// the cache does not hold: the provider has likely rotated its keys.
pub async fn refresh(provider: &IdentityProvider) -> Result<ProviderKeys, Reason> {
    Ok(parse_jwks(&fetch(provider).await?.jwks, Instant::now()))
}

/// Forget a provider's cached keys. Called when its row is deleted so a
/// provider recreated under the same name does not keep the old keys.
pub fn evict(provider: &IdentityProvider) {
    lock_cache().remove(&provider.meta.id.to_string());
}

/// Discovery, then the JWKS, stored under the provider and returned. Any
/// failure is one reason: the caller's answer never says which step broke.
async fn fetch(provider: &IdentityProvider) -> Result<Cached, Reason> {
    let base = match discovery_url(&provider.spec.issuer) {
        Ok(url) => url,
        Err(_) => return Err(Reason::BadIssuer),
    };
    let client = match http_client() {
        Ok(c) => c,
        Err(_) => return Err(Reason::KeysUnavailable),
    };
    let doc: Discovery = match get_json(&client, &base).await {
        Ok(d) => d,
        Err(_) => return Err(Reason::KeysUnavailable),
    };
    let jwks_url = match allowed_url(&doc.jwks_uri) {
        Ok(url) => url,
        Err(_) => return Err(Reason::KeysUnavailable),
    };
    let text = match get_text(&client, &jwks_url).await {
        Ok(t) => t,
        Err(_) => return Err(Reason::KeysUnavailable),
    };
    // Parse once here so a stored JWKS is always usable; a set with no key we
    // accept is a failure, not a cacheable empty answer.
    if parse_jwks(&text, Instant::now()).keys.is_empty() {
        return Err(Reason::KeysUnavailable);
    }
    let cached = Cached {
        jwks: text,
        fetched_at: Instant::now(),
    };
    lock_cache().insert(provider.meta.id.to_string(), cached.clone());
    Ok(cached)
}

/// Turn JWKS text into the keys we accept. Unparsable text yields no keys,
/// which the caller treats as "not accepted".
fn parse_jwks(text: &str, fetched_at: Instant) -> ProviderKeys {
    let jwks: Jwks = match serde_json::from_str(text) {
        Ok(j) => j,
        Err(_) => {
            return ProviderKeys {
                keys: Vec::new(),
                fetched_at,
            }
        }
    };
    ProviderKeys {
        keys: jwks.keys.iter().filter_map(to_decoding_key).collect(),
        fetched_at,
    }
}

#[derive(serde::Deserialize)]
struct Discovery {
    jwks_uri: String,
}

#[derive(serde::Deserialize)]
struct Jwks {
    #[serde(default)]
    keys: Vec<Jwk>,
}

#[derive(serde::Deserialize)]
struct Jwk {
    #[serde(rename = "kty")]
    kty: String,
    #[serde(default)]
    kid: Option<String>,
    #[serde(default)]
    alg: Option<String>,
    #[serde(rename = "use", default)]
    use_: Option<String>,
    #[serde(default)]
    n: Option<String>,
    #[serde(default)]
    e: Option<String>,
    #[serde(default)]
    x: Option<String>,
    #[serde(default)]
    y: Option<String>,
    #[serde(default)]
    crv: Option<String>,
}

async fn get_json<T: serde::de::DeserializeOwned>(
    client: &reqwest::Client,
    url: &str,
) -> Result<T, ()> {
    let res = client.get(url).send().await.map_err(|_| ())?;
    if !res.status().is_success() {
        return Err(());
    }
    res.json::<T>().await.map_err(|_| ())
}

async fn get_text(client: &reqwest::Client, url: &str) -> Result<String, ()> {
    let res = client.get(url).send().await.map_err(|_| ())?;
    if !res.status().is_success() {
        return Err(());
    }
    res.text().await.map_err(|_| ())
}

fn http_client() -> Result<reqwest::Client, ()> {
    reqwest::Client::builder()
        .timeout(FETCH_TIMEOUT)
        .connect_timeout(FETCH_TIMEOUT)
        .build()
        .map_err(|_| ())
}

/// `<issuer>/.well-known/openid-configuration`, or `Err` when the issuer is
/// not an allowed URL.
fn discovery_url(issuer: &str) -> Result<String, ()> {
    allowed_url(issuer).map(|issuer| format!("{issuer}{WELL_KNOWN}"))
}

/// Whether an issuer is a URL gate may fetch. The create handler asks this so a
/// provider that could never work is refused before it is stored.
pub fn issuer_is_reachable(issuer: &str) -> bool {
    allowed_url(issuer).is_ok()
}

/// `url` only if it is HTTPS, or HTTP to a loopback host (for tests).
fn allowed_url(url: &str) -> Result<String, ()> {
    let url = url.trim().trim_end_matches('/');
    let (https, rest) = match url.split_once("://") {
        Some(("https", rest)) => (true, rest),
        Some(("http", rest)) => (false, rest),
        _ => return Err(()),
    };
    let authority = rest
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default()
        .rsplit_once('@')
        .map_or(rest, |(_, host)| host);
    // An IPv6 literal arrives percent-encoded in a URL authority (`%5B::1%5D`);
    // decode it before looking at the host.
    let authority = percent_encoding::percent_decode_str(authority)
        .decode_utf8()
        .map(|s| s.into_owned())
        .unwrap_or_else(|_| authority.to_owned());
    if authority.is_empty() {
        return Err(());
    }
    if https {
        return Ok(url.to_owned());
    }
    if is_loopback(&authority) {
        return Ok(url.to_owned());
    }
    Err(())
}

/// Whether an authority (host, host:port, or `[ipv6]:port`) names the loopback
/// address. Only hosts we would fetch over plain HTTP for tests are allowed.
fn is_loopback(authority: &str) -> bool {
    if let Some(bracketed) = authority.strip_prefix('[') {
        let inner = bracketed.split(']').next().unwrap_or_default();
        return inner
            .parse::<std::net::Ipv6Addr>()
            .is_ok_and(|ip| ip.is_loopback());
    }
    let host = authority.split(':').next().unwrap_or_default();
    host == "127.0.0.1" || host == "localhost"
}

/// A JWKS entry to a verification key, or `None` to drop it.
fn to_decoding_key(jwk: &Jwk) -> Option<ProviderKey> {
    if jwk.use_.as_deref().is_some_and(|u| u != "sig") {
        return None;
    }
    // JWK members are base64url without padding (RFC 7518). Decode them to raw
    // bytes and hand the verifier the bytes, not a re-encoded string: the two
    // base64 alphabets differ on `+`/`/` vs `-`/`_`, and re-encoding would drop
    // or corrupt keys whose components use either alphabet's distinctive bytes.
    let engine = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let alg = match jwk.kty.as_str() {
        "RSA" => Algorithm::RS256,
        "EC" => Algorithm::ES256,
        // `oct` (a shared secret) and anything else are never verification
        // keys here.
        _ => return None,
    };
    // A key that names an algorithm we do not accept is not trusted to be used
    // only for the one we would pick for it.
    if let Some(declared) = jwk.alg.as_deref() {
        let wants = match declared {
            "RS256" => Algorithm::RS256,
            "ES256" => Algorithm::ES256,
            _ => return None,
        };
        if wants != alg {
            return None;
        }
    }
    let key = match alg {
        Algorithm::RS256 => {
            let n = engine.decode(jwk.n.as_deref()?).ok()?;
            let e = engine.decode(jwk.e.as_deref()?).ok()?;
            DecodingKey::from_rsa_raw_components(&n, &e)
        }
        Algorithm::ES256 => {
            // ES256 is P-256. A key on another curve is dropped rather than
            // guessed at.
            if jwk.crv.as_deref().is_some_and(|c| c != "P-256") {
                return None;
            }
            let x = engine.decode(jwk.x.as_deref()?).ok()?;
            let y = engine.decode(jwk.y.as_deref()?).ok()?;
            if x.len() != 32 || y.len() != 32 {
                return None;
            }
            // An uncompressed SEC1 point: 0x04 || x || y. `from_ec_raw_components`
            // is not exposed, so the DER the pem path would build is assembled
            // here from the raw point.
            let mut point = Vec::with_capacity(65);
            point.push(0x04);
            point.extend_from_slice(&x);
            point.extend_from_slice(&y);
            DecodingKey::from_ec_der(&point)
        }
        _ => return None,
    };
    Some(ProviderKey {
        kid: jwk.kid.clone(),
        alg,
        key,
    })
}

/// Build a key set from JWKS text, for tests of the refetch decision.
#[cfg(test)]
pub fn parse_jwks_for_test(text: &str, at: Instant) -> ProviderKeys {
    parse_jwks(text, at)
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;

    #[test]
    fn https_is_allowed_and_loopback_http_is() {
        assert!(allowed_url("https://id.example/").is_ok());
        assert_eq!(
            allowed_url("https://id.example").unwrap(),
            "https://id.example"
        );
        assert!(allowed_url("http://127.0.0.1:8080").is_ok());
        assert!(allowed_url("http://localhost:1234/x").is_ok());
        // The IPv6 loopback in both its compressed and its full form.
        assert!(allowed_url("http://[::1]:8080").is_ok());
        assert!(allowed_url("http://[::1]").is_ok());
        assert!(allowed_url("http://[0:0:0:0:0:0:0:1]:9").is_ok());
        // A non-loopback IPv6 literal is refused over plain http.
        assert!(allowed_url("http://[::2]:9").is_err());
    }

    #[test]
    fn plain_http_elsewhere_is_refused() {
        assert!(allowed_url("http://id.example").is_err());
        assert!(allowed_url("http://10.0.0.5").is_err());
        assert!(allowed_url("http://127.0.0.1.evil.example").is_err());
        assert!(allowed_url("ftp://id.example").is_err());
        assert!(allowed_url("not a url").is_err());
        assert!(allowed_url("https://").is_err());
    }

    #[test]
    fn discovery_appends_the_well_known_path() {
        assert_eq!(
            discovery_url("https://id.example").unwrap(),
            "https://id.example/.well-known/openid-configuration"
        );
        assert_eq!(
            discovery_url("https://id.example/").unwrap(),
            "https://id.example/.well-known/openid-configuration"
        );
    }

    #[test]
    fn a_symmetric_or_unusable_key_is_dropped() {
        let jwks = r#"{"keys":[
            {"kty":"oct","kid":"k","alg":"HS256","k":"AAA"},
            {"kty":"RSA","kid":"enc","use":"enc","n":"AAA","e":"AQAB"},
            {"kty":"RSA","kid":"badalg","alg":"RS512","n":"AAA","e":"AQAB"}
        ]}"#;
        let keys = parse_jwks(jwks, Instant::now());
        assert!(keys.keys.is_empty(), "kept {} keys", keys.keys.len());
    }

    #[test]
    fn rsa_components_are_read_as_base64url() {
        // A modulus whose base64url form contains `-` and `_` (bytes 0xFB,0xFF,
        // 0xBF,0xFE: 3 bytes -> 4 chars `+_-~` in url-safe become `-_-_`). Under
        // the standard alphabet these would be `+`/`/`, which `b64_decode` (a
        // base64url decoder) rejects, so reading as standard base64 would drop
        // the key. `e` is the usual 65537.
        let n_url = URL_SAFE_NO_PAD.encode([0xFBu8, 0xFF, 0xBF, 0xFE]);
        assert!(
            n_url.contains('-') || n_url.contains('_'),
            "fixture must exercise the url-safe alphabet: {n_url}"
        );
        let jwks = format!(
            r#"{{"keys":[{{"kty":"RSA","kid":"k","alg":"RS256","n":"{n_url}","e":"AQAB"}}]}}"#
        );
        let keys = parse_jwks(&jwks, Instant::now());
        assert_eq!(keys.keys.len(), 1, "key with url-safe modulus was dropped");
        assert_eq!(keys.keys[0].kid.as_deref(), Some("k"));
    }
}
