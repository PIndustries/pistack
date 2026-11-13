// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Signed bearer tokens.
//!
//! A token is a JWT (HS256) whose claims are exactly the `Caller` fields
//! plus `iat`/`exp`/`jti`. It is signed with one of a small set of random
//! 32-byte secrets kept in the store's `meta` table ([`META_SIGNING_KEYS`]):
//! the newest signs, and its id travels in the token header as `kid`.
//! [`rotate`] adds a key and keeps the one before it verifying until the
//! longest-lived token signed with it has expired; [`retire`] drops the
//! older keys at once.
//!
//! A cloud started before keys had ids holds one secret under
//! [`META_JWT_SECRET`]. It is used as the only key of the set, under an id
//! worked out from the secret, until the set is saved.
//!
//! The secret servers recognise each other with ([`peer_secret`]) is kept
//! apart from the signing keys, so rotating them does not touch it.

use crate::types::{ApiKey, Principal};
use axum::{
    extract::{FromRef, FromRequestParts, Request, State},
    http::{header, request::Parts, HeaderMap},
    middleware::Next,
    response::{IntoResponse, Response},
};
use base64::Engine;
use chrono::{DateTime, Duration, Utc};
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation};
use pistack_store::Store;
use pistack_types::{ApiError, ApiResult, Caller, Id};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Store meta key holding the one base64 HS256 secret of a cloud started
/// before keys had ids. Deleted once that key is retired.
pub const META_JWT_SECRET: &str = "gate/jwt_secret";
/// Store meta key holding the [`KeySet`].
pub const META_SIGNING_KEYS: &str = "gate/signing_keys";
/// Store meta key holding the cluster peer secret (see [`peer_secret`]).
pub const META_PEER_SECRET: &str = "cluster/peer_secret";
/// Default token lifetime.
pub const DEFAULT_TTL: Duration = Duration::hours(12);
/// Longest lifetime a login may ask for.
pub const MAX_TTL: Duration = Duration::days(7);
/// Clock skew tolerated on `exp`.
const LEEWAY_SECS: u64 = 5;

/// JWT claims. The `Caller` fields are flattened so a token is the caller.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Claims {
    /// Principal id.
    pub sub: Id,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<Id>,
    #[serde(default)]
    pub roles: Vec<String>,
    #[serde(default)]
    pub system: bool,
    /// API key the login used, when it used one. The token dies with it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<Id>,
    pub iat: i64,
    pub exp: i64,
    pub jti: String,
}

impl Claims {
    pub fn caller(&self) -> Caller {
        Caller {
            principal: self.sub.clone(),
            principal_name: self.name.clone(),
            project: self.project.clone(),
            roles: self.roles.clone(),
            system: self.system,
        }
    }
    pub fn expires_at(&self) -> DateTime<Utc> {
        DateTime::from_timestamp(self.exp, 0).unwrap_or_default()
    }
}

/// One signing key. The secret is base64 and is never shown.
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SigningKey {
    pub kid: String,
    secret: String,
    /// When the key was made. Unknown for the key a cloud started with
    /// before keys had ids.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<DateTime<Utc>>,
    /// Set when a newer key replaced this one. The key verifies until
    /// then and never again after.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retire_at: Option<DateTime<Utc>>,
}

impl std::fmt::Debug for SigningKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SigningKey")
            .field("kid", &self.kid)
            .field("retire_at", &self.retire_at)
            .finish_non_exhaustive()
    }
}

impl SigningKey {
    fn new(secret: String, now: DateTime<Utc>, taken: &[SigningKey]) -> SigningKey {
        let kid = loop {
            let mut b = [0u8; 4];
            rand::rngs::OsRng.fill_bytes(&mut b);
            let kid = hex(&b);
            if !taken.iter().any(|k| k.kid == kid) {
                break kid;
            }
        };
        SigningKey {
            kid,
            secret,
            created_at: Some(now),
            retire_at: None,
        }
    }

    fn live(&self, now: DateTime<Utc>) -> bool {
        self.retire_at.is_none_or(|t| t > now)
    }
}

/// What `GET /identity/signing-keys` says about a key.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SigningKeyInfo {
    pub kid: String,
    /// New tokens are signed with this one.
    pub current: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retire_at: Option<DateTime<Utc>>,
}

/// The signing keys of a cloud, oldest first. The last one signs.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct KeySet {
    keys: Vec<SigningKey>,
}

impl KeySet {
    /// The set a cloud has while it still holds only the one secret from
    /// before keys had ids. The id is worked out from the secret, so every
    /// server names the key the same way without writing anything.
    fn legacy(secret_b64: &str) -> KeySet {
        use sha2::Digest;
        let mut h = sha2::Sha256::new();
        h.update(b"pistack signing key id\0");
        h.update(secret_b64.as_bytes());
        KeySet {
            keys: vec![SigningKey {
                kid: hex(&h.finalize()[..4]),
                secret: secret_b64.to_owned(),
                created_at: None,
                retire_at: None,
            }],
        }
    }

    fn fresh(now: DateTime<Utc>) -> KeySet {
        KeySet {
            keys: vec![SigningKey::new(random_secret(), now, &[])],
        }
    }

    /// The set as the store has it: the saved set, else the one made from
    /// the old single secret, else nothing.
    pub fn load(store: &Store) -> anyhow::Result<Option<KeySet>> {
        if let Some(set) = store.get_meta::<KeySet>(META_SIGNING_KEYS)? {
            if set.keys.is_empty() {
                anyhow::bail!("{META_SIGNING_KEYS} holds no key");
            }
            return Ok(Some(set));
        }
        Ok(store
            .get_meta::<String>(META_JWT_SECRET)?
            .map(|s| KeySet::legacy(&s)))
    }

    /// Id of the key that signs new tokens.
    pub fn current_kid(&self) -> &str {
        self.keys.last().map(|k| k.kid.as_str()).unwrap_or_default()
    }

    /// Every key, without its secret.
    pub fn describe(&self) -> Vec<SigningKeyInfo> {
        let current = self.current_kid();
        self.keys
            .iter()
            .map(|k| SigningKeyInfo {
                kid: k.kid.clone(),
                current: k.kid == current,
                created_at: k.created_at,
                retire_at: k.retire_at,
            })
            .collect()
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn random_secret() -> String {
    let mut buf = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut buf);
    base64::engine::general_purpose::STANDARD.encode(buf)
}

/// The secret control-plane servers prove membership with (see
/// `keep::peer`). It is made once and never changes with the signing keys.
///
/// A cloud from before it existed has none saved. There the value is
/// worked out from the old single signing secret exactly as servers always
/// did, and that same value is what gets saved, so servers on the build
/// before and after agree on it.
pub fn peer_secret(store: &Store) -> Option<String> {
    use hmac::Mac;
    if let Ok(Some(saved)) = store.get_meta::<String>(META_PEER_SECRET) {
        return Some(saved);
    }
    let secret = store.get_meta::<String>(META_JWT_SECRET).ok()??;
    let mut mac = hmac::Hmac::<sha2::Sha256>::new_from_slice(secret.as_bytes()).ok()?;
    mac.update(b"pistack cluster peer v1");
    Some(hex(&mac.finalize().into_bytes()))
}

/// Serialises read-modify-write of the key set within this process. Only
/// the leader writes it.
static SET_WRITES: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Save `set` together with the peer secret, in one transaction. Once the
/// old single secret names no key in the set it is deleted: nothing reads
/// it any more.
fn save_set(store: &Store, set: &KeySet) -> anyhow::Result<()> {
    let peer = peer_secret(store).unwrap_or_else(|| {
        let mut b = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut b);
        hex(&b)
    });
    store.put_meta_pair((META_SIGNING_KEYS, set), (META_PEER_SECRET, &peer))?;
    if let Some(old) = store.get_meta::<String>(META_JWT_SECRET)? {
        if !set.keys.iter().any(|k| k.secret == old) {
            store.delete_meta(META_JWT_SECRET)?;
        }
    }
    Ok(())
}

/// Save the key set and the peer secret if the store does not hold them
/// yet. The leader calls this after an upgrade; it changes no key.
pub fn ensure_saved(store: &Store) -> anyhow::Result<()> {
    let _g = SET_WRITES.lock().unwrap_or_else(|e| e.into_inner());
    let saved = store.get_meta::<KeySet>(META_SIGNING_KEYS)?.is_some()
        && store.get_meta::<String>(META_PEER_SECRET)?.is_some();
    if saved {
        return Ok(());
    }
    match KeySet::load(store)? {
        Some(set) => save_set(store, &set),
        None => Ok(()),
    }
}

/// How long a replaced key keeps verifying: the longest a token can live,
/// plus the clock skew allowed on `exp`.
pub fn retire_delay() -> Duration {
    MAX_TTL + Duration::seconds(LEEWAY_SECS as i64)
}

/// Make a new signing key and sign with it from now on. The key it
/// replaces verifies until `now + retire_delay()`, so tokens already out
/// stay good for their lifetime. Keys whose time has passed are dropped.
pub fn rotate(store: &Store, now: DateTime<Utc>) -> anyhow::Result<KeySet> {
    let _g = SET_WRITES.lock().unwrap_or_else(|e| e.into_inner());
    let mut set =
        KeySet::load(store)?.ok_or_else(|| anyhow::anyhow!("no token signing key to rotate"))?;
    set.keys.retain(|k| k.live(now));
    for k in &mut set.keys {
        k.retire_at.get_or_insert(now + retire_delay());
    }
    let next = SigningKey::new(random_secret(), now, &set.keys);
    set.keys.push(next);
    save_set(store, &set)?;
    Ok(set)
}

/// Drop every key but the current one, now. Tokens signed with a dropped
/// key stop verifying at once. Returns the set and the ids dropped.
pub fn retire(store: &Store) -> anyhow::Result<(KeySet, Vec<String>)> {
    let _g = SET_WRITES.lock().unwrap_or_else(|e| e.into_inner());
    let mut set =
        KeySet::load(store)?.ok_or_else(|| anyhow::anyhow!("no token signing key to retire"))?;
    let keep = set.keys.len().saturating_sub(1);
    let dropped: Vec<String> = set.keys.drain(..keep).map(|k| k.kid).collect();
    if !dropped.is_empty() {
        save_set(store, &set)?;
    }
    Ok((set, dropped))
}

/// One key, ready to use.
struct BuiltKey {
    kid: String,
    retire_at: Option<DateTime<Utc>>,
    encoding: EncodingKey,
    decoding: DecodingKey,
}

struct Built {
    set: KeySet,
    keys: Vec<BuiltKey>,
}

impl Built {
    fn from_set(set: KeySet) -> anyhow::Result<Built> {
        let keys = set
            .keys
            .iter()
            .map(|k| {
                let secret = base64::engine::general_purpose::STANDARD
                    .decode(&k.secret)
                    .map_err(|e| anyhow::anyhow!("token signing key is not valid base64: {e}"))?;
                Ok(BuiltKey {
                    kid: k.kid.clone(),
                    retire_at: k.retire_at,
                    encoding: EncodingKey::from_secret(&secret),
                    decoding: DecodingKey::from_secret(&secret),
                })
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        Ok(Built { set, keys })
    }
}

/// The keys in use, re-read from the store when the saved set changes, so
/// a rotation made on one server reaches the others without a restart.
struct Ring {
    store: Option<Store>,
    built: std::sync::RwLock<Arc<Built>>,
}

impl Ring {
    fn current(&self) -> Arc<Built> {
        let have = self.built.read().unwrap_or_else(|e| e.into_inner()).clone();
        let Some(store) = &self.store else {
            return have;
        };
        // A store that cannot be read right now keeps the keys in hand.
        let Ok(Some(set)) = KeySet::load(store) else {
            return have;
        };
        if set == have.set {
            return have;
        }
        match Built::from_set(set) {
            Ok(b) => {
                let b = Arc::new(b);
                *self.built.write().unwrap_or_else(|e| e.into_inner()) = b.clone();
                tracing::info!(current = b.set.current_kid(), "token signing keys reloaded");
                b
            }
            Err(e) => {
                tracing::error!(error = %e, "cannot use the saved token signing keys");
                have
            }
        }
    }
}

/// Signing and verifying key material. One per cloud.
#[derive(Clone)]
pub struct Keys {
    ring: Arc<Ring>,
}

impl Keys {
    /// Load the keys from the store, generating and persisting one if
    /// this is the first start.
    pub fn load_or_create(store: &Store) -> anyhow::Result<Keys> {
        let set = match KeySet::load(store)? {
            Some(set) => set,
            None => {
                let _g = SET_WRITES.lock().unwrap_or_else(|e| e.into_inner());
                match KeySet::load(store)? {
                    Some(set) => set,
                    None => {
                        let set = KeySet::fresh(Utc::now());
                        save_set(store, &set)?;
                        tracing::info!("generated new token signing key");
                        set
                    }
                }
            }
        };
        Ok(Keys {
            ring: Arc::new(Ring {
                store: Some(store.clone()),
                built: std::sync::RwLock::new(Arc::new(Built::from_set(set)?)),
            }),
        })
    }

    pub fn from_secret(secret: &[u8]) -> Keys {
        let b64 = base64::engine::general_purpose::STANDARD.encode(secret);
        let set = KeySet::legacy(&b64);
        let keys = vec![BuiltKey {
            kid: set.current_kid().to_owned(),
            retire_at: None,
            encoding: EncodingKey::from_secret(secret),
            decoding: DecodingKey::from_secret(secret),
        }];
        Keys {
            ring: Arc::new(Ring {
                store: None,
                built: std::sync::RwLock::new(Arc::new(Built { set, keys })),
            }),
        }
    }

    pub fn verifier(&self) -> Verifier {
        Verifier::over(self.ring.clone())
    }

    /// Id of the key new tokens are signed with.
    pub fn current_kid(&self) -> String {
        self.ring.current().set.current_kid().to_owned()
    }

    /// Sign a token for `caller` valid for `ttl` from now.
    pub fn sign(&self, caller: &Caller, ttl: Duration) -> ApiResult<(String, Claims)> {
        self.sign_for_key(caller, None, ttl)
    }

    /// As [`Keys::sign`], for a login made with API key `key`.
    pub fn sign_for_key(
        &self,
        caller: &Caller,
        key: Option<Id>,
        ttl: Duration,
    ) -> ApiResult<(String, Claims)> {
        let now = Utc::now();
        self.sign_claims(caller, key, now, now + ttl)
    }

    /// Sign with explicit timestamps (tests use this to make expired tokens).
    pub fn sign_at(
        &self,
        caller: &Caller,
        issued: DateTime<Utc>,
        expires: DateTime<Utc>,
    ) -> ApiResult<(String, Claims)> {
        self.sign_claims(caller, None, issued, expires)
    }

    fn sign_claims(
        &self,
        caller: &Caller,
        key: Option<Id>,
        issued: DateTime<Utc>,
        expires: DateTime<Utc>,
    ) -> ApiResult<(String, Claims)> {
        let claims = Claims {
            sub: caller.principal.clone(),
            name: caller.principal_name.clone(),
            project: caller.project.clone(),
            roles: caller.roles.clone(),
            system: caller.system,
            key,
            iat: issued.timestamp(),
            exp: expires.timestamp(),
            jti: uuid::Uuid::now_v7().to_string(),
        };
        let built = self.ring.current();
        let signer = built
            .keys
            .last()
            .ok_or_else(|| ApiError::internal("no token signing key"))?;
        // A server on the build before key ids ignores `kid` and checks
        // the one secret it knows, which is this key until the first
        // rotation.
        let mut header = Header::new(Algorithm::HS256);
        header.kid = Some(signer.kid.clone());
        let token = jsonwebtoken::encode(&header, &claims, &signer.encoding)
            .map_err(|e| ApiError::internal(format!("signing token: {e}")))?;
        Ok((token, claims))
    }
}

/// Verifies tokens. Cheap to clone; the gateway holds one and checks every
/// request with it.
///
/// A token names its key with `kid` and is checked against that key while
/// the key is in the set and not past its retirement. A token with no
/// `kid` comes from before keys had ids and is checked against the oldest
/// key.
///
/// One built with [`Verifier::from_store`] also reads the store on every
/// check: the principal must still exist and be enabled, the token must
/// not be older than the principal's "tokens before this are invalid"
/// stamp, the API key the login used must still exist and be unexpired,
/// and the roles are the ones bound now, not the ones bound at login. So
/// disabling a user, deleting a key or removing a binding takes effect on
/// the next request. One built from a bare secret checks the signature and
/// expiry only.
#[derive(Clone)]
pub struct Verifier {
    ring: Arc<Ring>,
    validation: Arc<Validation>,
    store: Option<Store>,
}

impl Verifier {
    fn over(ring: Arc<Ring>) -> Verifier {
        let mut validation = Validation::new(Algorithm::HS256);
        validation.leeway = LEEWAY_SECS;
        validation.validate_exp = true;
        validation.set_required_spec_claims(&["exp", "sub"]);
        Verifier {
            ring,
            validation: Arc::new(validation),
            store: None,
        }
    }

    pub fn from_secret(secret: &[u8]) -> Verifier {
        Keys::from_secret(secret).verifier()
    }

    /// Load from the store (the gateway does this once at boot). Fails if
    /// `gate` has not generated a key yet.
    pub fn from_store(store: &Store) -> anyhow::Result<Verifier> {
        Ok(Keys::load_or_create(store)?.verifier().with_store(store))
    }

    /// Also check each token against `store`.
    pub fn with_store(mut self, store: &Store) -> Verifier {
        self.store = Some(store.clone());
        self
    }

    pub fn verify(&self, token: &str) -> ApiResult<Caller> {
        let claims = self.claims(token)?;
        match &self.store {
            Some(store) => live_caller(store, &claims),
            None => Ok(claims.caller()),
        }
    }

    pub fn claims(&self, token: &str) -> ApiResult<Claims> {
        self.claims_at(token, Utc::now())
    }

    /// [`Verifier::claims`] with the time keys are retired against given,
    /// so a test can stand after a key's retirement.
    pub fn claims_at(&self, token: &str, now: DateTime<Utc>) -> ApiResult<Claims> {
        let invalid = || ApiError::unauthorized("invalid token");
        let header = jsonwebtoken::decode_header(token).map_err(|_| invalid())?;
        let built = self.ring.current();
        let live = |k: &&BuiltKey| k.retire_at.is_none_or(|t| t > now);
        let key = match &header.kid {
            Some(kid) => built.keys.iter().filter(live).find(|k| &k.kid == kid),
            None => built.keys.iter().find(live),
        }
        .ok_or_else(invalid)?;
        jsonwebtoken::decode::<Claims>(token, &key.decoding, &self.validation)
            .map(|d| d.claims)
            .map_err(|e| {
                use jsonwebtoken::errors::ErrorKind as K;
                match e.kind() {
                    K::ExpiredSignature => ApiError::unauthorized("token expired"),
                    _ => invalid(),
                }
            })
    }

    /// Pull the bearer token out of request parts and verify it.
    pub fn caller_from_parts(&self, parts: &Parts) -> ApiResult<Caller> {
        if let Some(c) = parts.extensions.get::<Caller>() {
            return Ok(c.clone());
        }
        let token = bearer(parts)?;
        self.verify(token)
    }
}

/// The caller a token stands for as the store has it now. Any reason the
/// token no longer holds is the same 401, so a probe learns nothing.
fn live_caller(store: &Store, claims: &Claims) -> ApiResult<Caller> {
    let revoked = || ApiError::unauthorized("token revoked");
    let internal = |e: pistack_store::StoreError| ApiError::internal(e.to_string());
    let principal = store
        .collection::<Principal>()
        .try_get(&claims.sub)
        .map_err(internal)?
        .filter(|p| p.spec.enabled)
        .ok_or_else(revoked)?;
    // `iat` is in whole seconds, so a token from the second of the stamp
    // is taken as older than it.
    if let Some(stamp) = principal.status.tokens_not_before {
        if claims.iat <= stamp.timestamp() {
            return Err(revoked());
        }
    }
    if let Some(key) = &claims.key {
        store
            .collection::<ApiKey>()
            .try_get(key)
            .map_err(internal)?
            .filter(|k| k.spec.principal == principal.meta.id && !k.is_expired(Utc::now()))
            .ok_or_else(revoked)?;
    }
    crate::api::caller_for(store, &principal, claims.project.clone()).map_err(|_| revoked())
}

/// `pub fn verify(token)` convenience when a `Verifier` is in hand.
pub fn verify(verifier: &Verifier, token: &str) -> ApiResult<Caller> {
    verifier.verify(token)
}

fn bearer(parts: &Parts) -> ApiResult<&str> {
    bearer_in(&parts.headers)
}

/// The bearer token in a set of request headers.
pub fn bearer_in(headers: &HeaderMap) -> ApiResult<&str> {
    let mut auths = headers.get_all(header::AUTHORIZATION).iter();
    let raw = auths
        .next()
        .ok_or_else(|| ApiError::unauthorized("missing Authorization header"))?;
    if auths.next().is_some() {
        return Err(ApiError::unauthorized("multiple Authorization headers"));
    }
    let raw = raw
        .to_str()
        .map_err(|_| ApiError::unauthorized("malformed Authorization header"))?;
    let (scheme, token) = raw
        .split_once(' ')
        .ok_or_else(|| ApiError::unauthorized("expected 'Bearer <token>'"))?;
    if !scheme.eq_ignore_ascii_case("bearer") || token.trim().is_empty() {
        return Err(ApiError::unauthorized("expected 'Bearer <token>'"));
    }
    Ok(token.trim())
}

/// Axum extractor: the verified caller of this request.
///
/// Works in two modes. If the gateway's `attach_caller` middleware already
/// ran, the `Caller` is read from request extensions. Otherwise the
/// `Authorization: Bearer` header is verified with the `Verifier` obtained
/// from router state via `FromRef`.
#[derive(Clone, Debug)]
pub struct AuthCaller(pub Caller);

impl<S> FromRequestParts<S> for AuthCaller
where
    S: Send + Sync,
    Verifier: FromRef<S>,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        require_caller(&Verifier::from_ref(state), parts)
            .await
            .map(AuthCaller)
    }
}

/// Resolve the caller for a request: extensions first, then the bearer token.
pub async fn require_caller(verifier: &Verifier, parts: &Parts) -> ApiResult<Caller> {
    verifier.caller_from_parts(parts)
}

/// Gateway middleware: `axum::middleware::from_fn_with_state(verifier, gate::attach_caller)`.
///
/// A present-but-invalid bearer token is rejected with 401. A missing header
/// passes through untouched so unauthenticated routes (login, discovery)
/// keep working; handlers that need a caller use `AuthCaller` and get 401
/// from the extractor.
///
/// On paths under `/silo`, an `Authorization: AWS4-HMAC-SHA256 <…>` header
/// (scheme token followed by a space) is left for silo's SigV4 verifier.
/// The same scheme on any other path is treated as an unrecognized bearer
/// scheme (401). Multiple `Authorization` headers are always rejected.
pub async fn attach_caller(State(verifier): State<Verifier>, req: Request, next: Next) -> Response {
    let auth_values: Vec<_> = req
        .headers()
        .get_all(header::AUTHORIZATION)
        .iter()
        .collect();
    if auth_values.len() > 1 {
        return ApiError::unauthorized("multiple Authorization headers").into_response();
    }
    if let Some(raw) = auth_values.first() {
        let Ok(s) = raw.to_str() else {
            return ApiError::unauthorized("malformed Authorization header").into_response();
        };
        if is_aws4_authorization(s) && path_under_silo(req.uri().path()) {
            return next.run(req).await;
        }
        // Outside /silo (or non-AWS4): fall through to bearer parsing → 401
        // for unrecognized schemes.
        let (parts, body) = req.into_parts();
        match verifier.caller_from_parts(&parts) {
            Ok(caller) => {
                // Which API key the token was issued for, for the audit log.
                let key = bearer(&parts)
                    .ok()
                    .and_then(|token| verifier.claims(token).ok())
                    .and_then(|claims| claims.key);
                let mut req = Request::from_parts(parts, body);
                req.extensions_mut().insert(caller);
                if let Some(key) = key {
                    req.extensions_mut().insert(crate::audit::TokenKey(key));
                }
                return next.run(req).await;
            }
            Err(e) => return e.into_response(),
        }
    }
    next.run(req).await
}

/// `Authorization` scheme is exactly `AWS4-HMAC-SHA256` followed by a space.
pub fn is_aws4_authorization(value: &str) -> bool {
    const PREFIX: &[u8] = b"AWS4-HMAC-SHA256 ";
    let v = value.as_bytes();
    v.len() >= PREFIX.len() && v[..PREFIX.len()].eq_ignore_ascii_case(PREFIX)
}

/// True for `/silo` and `/silo/…` (not `/siloevil`).
pub fn path_under_silo(path: &str) -> bool {
    path == "/silo" || path.starts_with("/silo/")
}

/// True when a `/silo` request carries SigV4 material that must not get a
/// synthetic bearer [`Caller`]: a well-formed `AWS4-HMAC-SHA256 ` header, or
/// a presigned query (`X-Amz-Algorithm=AWS4-HMAC-SHA256` / `X-Amz-Signature`).
///
/// Used by gateway `dev_admin` so `--dev` does not inject an admin over
/// SigV4 (which silo would then reject as bearer+SigV4). `attach_caller`
/// already leaves header SigV4 alone under `/silo`; presigned requests have
/// no `Authorization` so they always reach `dev_admin`.
pub fn silo_sigv4_skips_bearer(path: &str, headers: &HeaderMap, query: Option<&str>) -> bool {
    if !path_under_silo(path) {
        return false;
    }
    let auths: Vec<_> = headers.get_all(header::AUTHORIZATION).iter().collect();
    if auths.len() == 1 {
        if let Ok(s) = auths[0].to_str() {
            if is_aws4_authorization(s) {
                return true;
            }
        }
    }
    let Some(q) = query.filter(|s| !s.is_empty()) else {
        return false;
    };
    for pair in q.split('&') {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        // Clients send the algorithm unescaped; accept the literal form.
        if k.eq_ignore_ascii_case("X-Amz-Algorithm") && v.eq_ignore_ascii_case("AWS4-HMAC-SHA256") {
            return true;
        }
        if k.eq_ignore_ascii_case("X-Amz-Signature") && !v.is_empty() {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use pistack_types::kind;

    fn caller() -> Caller {
        Caller {
            principal: Id::new(kind::PRINCIPAL),
            principal_name: "alice".into(),
            project: Some(Id::new(kind::PROJECT)),
            roles: vec!["member".into()],
            system: false,
        }
    }

    #[test]
    fn aws4_scheme_requires_space() {
        assert!(is_aws4_authorization(
            "AWS4-HMAC-SHA256 Credential=x/20200101/pistack/s3/aws4_request, Signature=ab"
        ));
        assert!(is_aws4_authorization("aws4-hmac-sha256 Credential=x"));
        assert!(!is_aws4_authorization("AWS4-HMAC-SHA256garbage"));
        assert!(!is_aws4_authorization("AWS4-HMAC-SHA256"));
        assert!(!is_aws4_authorization("Bearer tok"));
        assert!(path_under_silo("/silo"));
        assert!(path_under_silo("/silo/bucket/key"));
        assert!(!path_under_silo("/identity/tokens"));
        assert!(!path_under_silo("/siloevil"));
    }

    #[test]
    fn silo_sigv4_skips_bearer_header_and_presign() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            "AWS4-HMAC-SHA256 Credential=x/20200101/pistack/s3/aws4_request, Signature=ab"
                .parse()
                .unwrap(),
        );
        assert!(silo_sigv4_skips_bearer("/silo/b/k", &headers, None));
        assert!(!silo_sigv4_skips_bearer("/identity/tokens", &headers, None));

        let empty = HeaderMap::new();
        assert!(silo_sigv4_skips_bearer(
            "/silo/b/k",
            &empty,
            Some("X-Amz-Algorithm=AWS4-HMAC-SHA256&X-Amz-Signature=abc"),
        ));
        assert!(silo_sigv4_skips_bearer(
            "/silo/b/k",
            &empty,
            Some("X-Amz-Signature=abc"),
        ));
        assert!(!silo_sigv4_skips_bearer(
            "/silo/b/k",
            &empty,
            Some("list-type=2")
        ));
        assert!(!silo_sigv4_skips_bearer(
            "/other",
            &empty,
            Some("X-Amz-Algorithm=AWS4-HMAC-SHA256&X-Amz-Signature=abc"),
        ));
    }

    #[test]
    fn sign_verify_roundtrip() {
        let keys = Keys::from_secret(b"0123456789abcdef0123456789abcdef");
        let c = caller();
        let (tok, claims) = keys.sign(&c, DEFAULT_TTL).unwrap();
        assert_eq!(claims.sub, c.principal);
        let back = keys.verifier().verify(&tok).unwrap();
        assert_eq!(back, c);
        let parsed = keys.verifier().claims(&tok).unwrap();
        assert!(parsed.expires_at() > Utc::now());
    }

    #[test]
    fn expired_token_rejected() {
        let keys = Keys::from_secret(b"0123456789abcdef0123456789abcdef");
        let now = Utc::now();
        let (tok, _) = keys
            .sign_at(
                &caller(),
                now - Duration::hours(2),
                now - Duration::hours(1),
            )
            .unwrap();
        let err = keys.verifier().verify(&tok).unwrap_err();
        assert_eq!(err.message, "token expired");
        assert_eq!(err.code, pistack_types::error::ErrorCode::Unauthorized);
    }

    #[test]
    fn wrong_key_and_garbage_rejected() {
        let a = Keys::from_secret(b"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
        let b = Keys::from_secret(b"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb");
        let (tok, _) = a.sign(&caller(), DEFAULT_TTL).unwrap();
        assert!(b.verifier().verify(&tok).is_err());
        assert!(a.verifier().verify("not.a.jwt").is_err());
    }

    #[test]
    fn keys_persist_in_store() {
        let store = Store::in_memory().unwrap();
        let k1 = Keys::load_or_create(&store).unwrap();
        let k2 = Keys::load_or_create(&store).unwrap();
        let (tok, _) = k1.sign(&caller(), DEFAULT_TTL).unwrap();
        assert!(k2.verifier().verify(&tok).is_ok());
    }

    /// A token as a server from before key ids signs it: no `kid`.
    fn token_without_kid(secret_b64: &str, c: &Caller) -> String {
        let secret = base64::engine::general_purpose::STANDARD
            .decode(secret_b64)
            .unwrap();
        let now = Utc::now();
        let claims = Claims {
            sub: c.principal.clone(),
            name: c.principal_name.clone(),
            project: c.project.clone(),
            roles: c.roles.clone(),
            system: c.system,
            key: None,
            iat: now.timestamp(),
            exp: (now + DEFAULT_TTL).timestamp(),
            jti: "old".into(),
        };
        jsonwebtoken::encode(
            &Header::new(Algorithm::HS256),
            &claims,
            &EncodingKey::from_secret(&secret),
        )
        .unwrap()
    }

    fn kid_of(token: &str) -> Option<String> {
        jsonwebtoken::decode_header(token).unwrap().kid
    }

    #[test]
    fn a_new_token_names_its_key() {
        let store = Store::in_memory().unwrap();
        let keys = Keys::load_or_create(&store).unwrap();
        let (tok, _) = keys.sign(&caller(), DEFAULT_TTL).unwrap();
        assert_eq!(kid_of(&tok), Some(keys.current_kid()));
        // A fresh cloud saves the set and never writes the old single secret.
        assert!(store
            .get_meta::<KeySet>(META_SIGNING_KEYS)
            .unwrap()
            .is_some());
        assert!(store.get_meta::<String>(META_JWT_SECRET).unwrap().is_none());
    }

    #[test]
    fn old_and_new_tokens_verify_across_a_rotation_and_a_retired_key_stops() {
        let store = Store::in_memory().unwrap();
        let keys = Keys::load_or_create(&store).unwrap();
        // A second server that loaded its keys before the rotation.
        let other = Keys::load_or_create(&store).unwrap().verifier();
        let c = caller();
        let (old, _) = keys.sign(&c, DEFAULT_TTL).unwrap();
        let first = keys.current_kid();

        let now = Utc::now();
        let set = rotate(&store, now).unwrap();
        assert_eq!(set.describe().len(), 2);
        assert_ne!(set.current_kid(), first);
        let (new, _) = keys.sign(&c, DEFAULT_TTL).unwrap();
        assert_eq!(kid_of(&new).as_deref(), Some(set.current_kid()));

        for v in [keys.verifier(), other.clone()] {
            assert_eq!(v.verify(&old).unwrap(), c);
            assert_eq!(v.verify(&new).unwrap(), c);
        }

        // Past its retirement time the old key no longer verifies, with
        // nothing written in between.
        let later = now + retire_delay() + Duration::seconds(1);
        assert!(other.claims_at(&old, now + MAX_TTL).is_ok());
        assert_eq!(
            other.claims_at(&old, later).unwrap_err().message,
            "invalid token"
        );
        assert!(other.claims_at(&new, later).is_ok());

        // Retiring by hand cuts it off at once.
        let (set, dropped) = retire(&store).unwrap();
        assert_eq!(dropped, vec![first]);
        assert_eq!(set.describe().len(), 1);
        for v in [keys.verifier(), other] {
            assert_eq!(v.verify(&old).unwrap_err().message, "invalid token");
            assert_eq!(v.verify(&new).unwrap(), c);
        }
        assert!(retire(&store).unwrap().1.is_empty());
    }

    #[test]
    fn a_rotation_drops_keys_whose_time_has_passed() {
        let store = Store::in_memory().unwrap();
        Keys::load_or_create(&store).unwrap();
        let t0 = Utc::now();
        rotate(&store, t0).unwrap();
        let set = rotate(&store, t0 + Duration::days(1)).unwrap();
        assert_eq!(set.describe().len(), 3);
        // The first key was due a week after t0, the second a week after
        // t0 + 1d; a rotation later than both leaves only its own two.
        let set = rotate(&store, t0 + Duration::days(9)).unwrap();
        assert_eq!(set.describe().len(), 2);
        assert_eq!(set.describe().iter().filter(|k| k.current).count(), 1);
    }

    #[test]
    fn a_cloud_from_before_key_ids_keeps_its_tokens_through_the_upgrade() {
        let store = Store::in_memory().unwrap();
        let legacy = random_secret();
        store.put_meta(META_JWT_SECRET, &legacy).unwrap();
        let c = caller();
        let before = token_without_kid(&legacy, &c);

        // Loading writes nothing: a follower must not write its replica.
        let keys = Keys::load_or_create(&store).unwrap();
        assert!(store
            .get_meta::<KeySet>(META_SIGNING_KEYS)
            .unwrap()
            .is_none());
        let again = Keys::load_or_create(&store).unwrap();
        assert_eq!(keys.current_kid(), again.current_kid());
        assert_eq!(keys.verifier().verify(&before).unwrap(), c);

        // What this build signs, a server on the build before verifies:
        // the same secret, and it does not look at `kid`.
        let (tok, _) = keys.sign(&c, DEFAULT_TTL).unwrap();
        assert!(kid_of(&tok).is_some());
        let mut plain = Validation::new(Algorithm::HS256);
        plain.set_required_spec_claims(&["exp", "sub"]);
        let secret = base64::engine::general_purpose::STANDARD
            .decode(&legacy)
            .unwrap();
        jsonwebtoken::decode::<Claims>(&tok, &DecodingKey::from_secret(&secret), &plain).unwrap();

        // Saving the set changes no key and leaves the old secret in
        // place for servers still on the build before.
        ensure_saved(&store).unwrap();
        ensure_saved(&store).unwrap();
        assert_eq!(keys.current_kid(), again.current_kid());
        assert_eq!(
            store
                .get_meta::<String>(META_JWT_SECRET)
                .unwrap()
                .as_deref(),
            Some(legacy.as_str())
        );

        // After a rotation a token without `kid` still verifies, against
        // the oldest key, until that key is retired.
        rotate(&store, Utc::now()).unwrap();
        assert_eq!(keys.verifier().verify(&before).unwrap(), c);
        assert_eq!(keys.verifier().verify(&tok).unwrap(), c);
        retire(&store).unwrap();
        assert!(keys.verifier().verify(&before).is_err());
        assert!(keys.verifier().verify(&tok).is_err());
        // The old single secret went with its key.
        assert!(store.get_meta::<String>(META_JWT_SECRET).unwrap().is_none());
    }

    #[test]
    fn the_peer_secret_does_not_move_with_the_signing_keys() {
        // A cloud from before: worked out from the old secret, then saved
        // as that same value.
        let store = Store::in_memory().unwrap();
        assert!(peer_secret(&store).is_none());
        store.put_meta(META_JWT_SECRET, &random_secret()).unwrap();
        let derived = peer_secret(&store).unwrap();
        assert_eq!(derived.len(), 64);
        assert!(store
            .get_meta::<String>(META_PEER_SECRET)
            .unwrap()
            .is_none());
        ensure_saved(&store).unwrap();
        assert_eq!(
            store
                .get_meta::<String>(META_PEER_SECRET)
                .unwrap()
                .as_deref(),
            Some(derived.as_str())
        );
        rotate(&store, Utc::now()).unwrap();
        assert_eq!(peer_secret(&store).unwrap(), derived);
        retire(&store).unwrap();
        assert!(store.get_meta::<String>(META_JWT_SECRET).unwrap().is_none());
        assert_eq!(peer_secret(&store).unwrap(), derived);

        // A rotation with nothing saved yet saves the secret with the set.
        let store = Store::in_memory().unwrap();
        store.put_meta(META_JWT_SECRET, &random_secret()).unwrap();
        let derived = peer_secret(&store).unwrap();
        rotate(&store, Utc::now()).unwrap();
        retire(&store).unwrap();
        assert_eq!(peer_secret(&store).unwrap(), derived);

        // A new cloud gets a random one with its first key.
        let fresh = Store::in_memory().unwrap();
        Keys::load_or_create(&fresh).unwrap();
        let made = peer_secret(&fresh).unwrap();
        assert_eq!(made.len(), 64);
        assert_ne!(made, derived);
        rotate(&fresh, Utc::now()).unwrap();
        assert_eq!(peer_secret(&fresh).unwrap(), made);
    }

    #[test]
    fn bearer_parsing() {
        let req = axum::http::Request::builder()
            .header("authorization", "Bearer abc")
            .body(())
            .unwrap();
        let (parts, _) = req.into_parts();
        assert_eq!(bearer(&parts).unwrap(), "abc");
        let req = axum::http::Request::builder()
            .header("authorization", "Basic abc")
            .body(())
            .unwrap();
        assert!(bearer(&req.into_parts().0).is_err());
        let req = axum::http::Request::builder().body(()).unwrap();
        assert!(bearer(&req.into_parts().0).is_err());
    }
}
