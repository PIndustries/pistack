// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Signed bearer tokens.
//!
//! A token is a JWT (HS256 for now; the key material is one random 32-byte
//! secret generated on first start and kept under the store's `meta` table)
//! whose claims are exactly the `Caller` fields plus `iat`/`exp`/`jti`.
//! Verifying one needs the key and nothing else: no store lookup, so the
//! gateway can do it on every request with a cheap, cloneable `Verifier`.

use axum::{
    extract::{FromRef, FromRequestParts, Request, State},
    http::{header, request::Parts},
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

/// Store meta key holding the base64 HS256 secret.
pub const META_JWT_SECRET: &str = "gate/jwt_secret";
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

/// Signing and verifying key material. One per cloud.
#[derive(Clone)]
pub struct Keys {
    encoding: Arc<EncodingKey>,
    verifier: Verifier,
}

impl Keys {
    /// Load the secret from the store, generating and persisting one if
    /// this is the first start.
    pub fn load_or_create(store: &Store) -> anyhow::Result<Keys> {
        let b64 = match store.get_meta::<String>(META_JWT_SECRET)? {
            Some(s) => s,
            None => {
                let mut buf = [0u8; 32];
                rand::rngs::OsRng.fill_bytes(&mut buf);
                let s = base64::engine::general_purpose::STANDARD.encode(buf);
                store.put_meta(META_JWT_SECRET, &s)?;
                tracing::info!("generated new token signing key");
                s
            }
        };
        let secret = base64::engine::general_purpose::STANDARD
            .decode(b64)
            .map_err(|e| anyhow::anyhow!("token signing key is not valid base64: {e}"))?;
        Ok(Keys::from_secret(&secret))
    }

    pub fn from_secret(secret: &[u8]) -> Keys {
        Keys {
            encoding: Arc::new(EncodingKey::from_secret(secret)),
            verifier: Verifier::from_secret(secret),
        }
    }

    pub fn verifier(&self) -> Verifier {
        self.verifier.clone()
    }

    /// Sign a token for `caller` valid for `ttl` from now.
    pub fn sign(&self, caller: &Caller, ttl: Duration) -> ApiResult<(String, Claims)> {
        let now = Utc::now();
        self.sign_at(caller, now, now + ttl)
    }

    /// Sign with explicit timestamps (tests use this to make expired tokens).
    pub fn sign_at(
        &self,
        caller: &Caller,
        issued: DateTime<Utc>,
        expires: DateTime<Utc>,
    ) -> ApiResult<(String, Claims)> {
        let claims = Claims {
            sub: caller.principal.clone(),
            name: caller.principal_name.clone(),
            project: caller.project.clone(),
            roles: caller.roles.clone(),
            system: caller.system,
            iat: issued.timestamp(),
            exp: expires.timestamp(),
            jti: uuid::Uuid::now_v7().to_string(),
        };
        let token = jsonwebtoken::encode(&Header::new(Algorithm::HS256), &claims, &self.encoding)
            .map_err(|e| ApiError::internal(format!("signing token: {e}")))?;
        Ok((token, claims))
    }
}

/// Verifies tokens. Cheap to clone; the gateway holds one and checks every
/// request with it. No store access.
#[derive(Clone)]
pub struct Verifier {
    decoding: Arc<DecodingKey>,
    validation: Arc<Validation>,
}

impl Verifier {
    pub fn from_secret(secret: &[u8]) -> Verifier {
        let mut validation = Validation::new(Algorithm::HS256);
        validation.leeway = LEEWAY_SECS;
        validation.validate_exp = true;
        validation.set_required_spec_claims(&["exp", "sub"]);
        Verifier {
            decoding: Arc::new(DecodingKey::from_secret(secret)),
            validation: Arc::new(validation),
        }
    }

    /// Load from the store (the gateway does this once at boot). Fails if
    /// `gate` has not generated a key yet.
    pub fn from_store(store: &Store) -> anyhow::Result<Verifier> {
        Ok(Keys::load_or_create(store)?.verifier())
    }

    pub fn verify(&self, token: &str) -> ApiResult<Caller> {
        Ok(self.claims(token)?.caller())
    }

    pub fn claims(&self, token: &str) -> ApiResult<Claims> {
        jsonwebtoken::decode::<Claims>(token, &self.decoding, &self.validation)
            .map(|d| d.claims)
            .map_err(|e| {
                use jsonwebtoken::errors::ErrorKind as K;
                match e.kind() {
                    K::ExpiredSignature => ApiError::unauthorized("token expired"),
                    _ => ApiError::unauthorized("invalid token"),
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

/// `pub fn verify(token)` convenience when a `Verifier` is in hand.
pub fn verify(verifier: &Verifier, token: &str) -> ApiResult<Caller> {
    verifier.verify(token)
}

fn bearer(parts: &Parts) -> ApiResult<&str> {
    let raw = parts
        .headers
        .get(header::AUTHORIZATION)
        .ok_or_else(|| ApiError::unauthorized("missing Authorization header"))?
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
/// from the extractor. An `Authorization: AWS4-HMAC-SHA256 …` header is
/// left for silo's SigV4 verifier (not treated as a gate bearer).
pub async fn attach_caller(State(verifier): State<Verifier>, req: Request, next: Next) -> Response {
    if let Some(raw) = req.headers().get(header::AUTHORIZATION) {
        let is_sigv4 = raw
            .as_bytes()
            .get(..16)
            .is_some_and(|p| p.eq_ignore_ascii_case(b"AWS4-HMAC-SHA256"));
        if is_sigv4 {
            return next.run(req).await;
        }
        let (parts, body) = req.into_parts();
        match verifier.caller_from_parts(&parts) {
            Ok(caller) => {
                let mut req = Request::from_parts(parts, body);
                req.extensions_mut().insert(caller);
                return next.run(req).await;
            }
            Err(e) => return e.into_response(),
        }
    }
    next.run(req).await
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
