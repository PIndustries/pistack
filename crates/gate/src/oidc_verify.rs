// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Checking an ID token once its provider's keys are in hand.
//!
//! The signature is verified against the single key the token names, for
//! `RS256` or `ES256` only; `none`, `HS*` and every other algorithm fail
//! before a key is consulted. Then the registered claims (`iss`, `aud`, `exp`,
//! `nbf`) are checked with sixty seconds of leeway, and the `email_verified`
//! rule is applied. The caller turns a `Reject` into the one refusal answer;
//! the variant names the reason for the audit record only.

use crate::oidc_claims::IdTokenClaims;
use crate::oidc_discovery::ProviderKeys;
use crate::oidc_error::Reason;
use crate::types::IdentityProvider;
use jsonwebtoken::{decode, decode_header, Algorithm, Validation};

/// Seconds of clock skew tolerated on `exp` and `nbf`.
pub const LEEWAY_SECS: u64 = 60;

/// Why the algorithm named in a token header is not one to verify against.
///
/// `none` has no signature and a shared-secret algorithm could be forged with
/// the public key the JWKS publishes; both are refused without consulting any
/// key.
fn check_alg(header_alg: Algorithm) -> Result<Algorithm, Reason> {
    match header_alg {
        Algorithm::RS256 | Algorithm::ES256 => Ok(header_alg),
        _ => Err(Reason::Signature),
    }
}

/// The `kid` from a token header, when it names one.
fn header_kid(token: &str) -> Option<String> {
    decode_header(token)
        .ok()
        .and_then(|h| h.kid)
        .filter(|k| !k.is_empty())
}

/// Decode the token with the provider's keys: the named key if present, else
/// the single key on offer. A `kid` the set does not hold means the provider
/// rotated its keys since the cache filled; the caller refetches once and
/// tries again. The decoder checks `iss`, `aud`, `exp` and `nbf` against the
/// wall clock with the leeway set here; gate adds no time rule of its own.
fn decode_with(
    token: &str,
    keys: &ProviderKeys,
    provider: &IdentityProvider,
) -> Result<IdTokenClaims, DecodeError> {
    let header_alg = match decode_header(token) {
        Ok(h) => h.alg,
        Err(_) => return Err(DecodeError::Bad(Reason::Signature)),
    };
    let alg = match check_alg(header_alg) {
        Ok(a) => a,
        Err(r) => return Err(DecodeError::Bad(r)),
    };
    let kid = header_kid(token);
    let key = match keys.find(kid.as_deref()) {
        Some(k) if k.alg == alg => k,
        // The token names a key we do not hold (or names none the set offers):
        // worth a single refetch, in case the provider just rotated its set.
        _ => return Err(DecodeError::UnknownKid),
    };
    let mut validation = Validation::new(alg);
    validation.set_audience(&[&provider.spec.client_id]);
    validation.set_issuer(&[&provider.spec.issuer]);
    validation.leeway = LEEWAY_SECS;
    // The audience may travel as a single string or a list; the decoder
    // accepts both. `iss`, `aud`, `exp` and `sub` are required — the four an ID
    // token is always required to carry, and `sub` is the half of the pair an
    // account is bound to. `nbf` is not: it is optional, so requiring it would
    // refuse valid tokens that simply do not set it. It is still honoured when a
    // token does carry it, which `validate_nbf` turns on: the decoder then
    // checks `nbf` against the clock only if the claim is present.
    validation.validate_nbf = true;
    validation.set_required_spec_claims(&["exp", "aud", "iss", "sub"]);
    match decode::<IdTokenClaims>(token, &key.key, &validation) {
        Ok(data) => Ok(data.claims),
        // A clock, issuer, audience or missing-required-claim error is a claims
        // failure; a bad key, algorithm or malformed token is a signature
        // failure. Both end in the one refusal answer; the reason is audit-only.
        Err(e) => {
            use jsonwebtoken::errors::ErrorKind::*;
            let reason = match *e.kind() {
                ExpiredSignature
                | ImmatureSignature
                | InvalidIssuer
                | InvalidAudience
                | MissingRequiredClaim(_) => Reason::Claims,
                _ => Reason::Signature,
            };
            Err(DecodeError::Bad(reason))
        }
    }
}

/// The outcome of the decode step: usable claims, a token naming a key we do
/// not have (worth a refetch), or a refusal with its reason.
enum DecodeError {
    UnknownKid,
    Bad(Reason),
}

/// Verify a token against the provider's keys and check the claims that are
/// gate's to check. On success returns the parsed claims.
pub fn verify(
    token: &str,
    keys: &ProviderKeys,
    provider: &IdentityProvider,
) -> Result<IdTokenClaims, Reason> {
    let claims = match decode_with(token, keys, provider) {
        Ok(claims) => claims,
        // `verify` is called after the caller has already refetched on an
        // unknown `kid`; a second miss is a plain refusal.
        Err(DecodeError::UnknownKid) | Err(DecodeError::Bad(Reason::Signature)) => {
            return Err(Reason::Signature)
        }
        Err(DecodeError::Bad(r)) => return Err(r),
    };
    check_email_verified(&claims, &provider.spec.name_claim)?;
    Ok(claims)
}

/// When the account is named by email and the token carries `email_verified`,
/// it must be true. A token that omits the claim is accepted; a provider that
/// sends it and says the address is unverified is refused.
fn check_email_verified(claims: &IdTokenClaims, name_claim: &str) -> Result<(), Reason> {
    if name_claim != "email" {
        return Ok(());
    }
    match claims.email_verified() {
        Some(true) | None => Ok(()),
        Some(false) => Err(Reason::EmailNotVerified),
    }
}

/// The value of the configured name claim, as a string. A missing or non-string
/// value leaves nothing to label a new account with.
pub fn subject_name(claims: &IdTokenClaims, name_claim: &str) -> Result<String, Reason> {
    claims
        .claim_str(name_claim)
        .map(str::to_owned)
        .filter(|s| !s.is_empty())
        .ok_or(Reason::NoSubject)
}

/// The label a new account takes from a name claim's value: a dotted email
/// becomes a name the store accepts. Local part lower-cased, characters outside
/// `[a-zA-Z0-9._-]` dropped, capped at the resource name length.
///
/// Nothing is invented when the value holds nothing legal: the result is empty
/// and the caller creates no account. A label, not a key — which account a token
/// signs in to is not a question a name answers at all.
pub fn account_name(raw: &str) -> String {
    let local = raw.split('@').next().unwrap_or(raw).to_ascii_lowercase();
    local
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        .take(63)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oidc_claims::IdTokenClaims;
    use base64::Engine as _;

    /// Parse a claims object carrying `iss`, `aud` and `exp`, with `extra`
    /// appended (which must begin with a comma).
    fn claims(extra: &str) -> IdTokenClaims {
        let text = format!(r#"{{"iss":"i","sub":"s","aud":"c","exp":1000{extra}}}"#);
        serde_json::from_str(&text).unwrap_or_else(|e| panic!("parse `{text}`: {e}"))
    }

    #[test]
    fn only_rs256_and_es256_are_verified_against() {
        // Every other algorithm the crate knows — HMAC and EdDSA — is refused
        // before a key is consulted. A `none` token never even reaches here: its
        // header fails to parse (see the `alg_none` end-to-end test).
        for alg in [
            Algorithm::HS256,
            Algorithm::HS384,
            Algorithm::HS512,
            Algorithm::RS384,
            Algorithm::RS512,
            Algorithm::PS256,
            Algorithm::EdDSA,
        ] {
            assert_eq!(check_alg(alg).err(), Some(Reason::Signature), "{alg:?}");
        }
        assert_eq!(check_alg(Algorithm::RS256).unwrap(), Algorithm::RS256);
        assert_eq!(check_alg(Algorithm::ES256).unwrap(), Algorithm::ES256);
    }

    #[test]
    fn a_none_algorithm_header_does_not_parse() {
        // jsonwebtoken has no `none` algorithm, so a `{"alg":"none"}` header is
        // rejected at the header read rather than reaching `check_alg`.
        let header = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(r#"{"alg":"none","typ":"JWT"}"#);
        assert!(decode_header(&format!("{header}.AAAA.BBBB")).is_err());
    }

    #[test]
    fn email_verified_is_required_only_for_an_email_name_claim() {
        assert!(check_email_verified(&claims(r#","email_verified":false"#), "email").is_err());
        assert!(check_email_verified(&claims(r#","email_verified":true"#), "email").is_ok());
        // No claim present: accepted.
        assert!(check_email_verified(&claims(""), "email").is_ok());
        // A non-email name claim is not held to the email rule.
        assert!(check_email_verified(&claims(r#","email_verified":false"#), "sub").is_ok());
    }

    #[test]
    fn the_name_claim_must_be_a_non_empty_string() {
        assert_eq!(
            subject_name(&claims(r#","email":"a@b.c""#), "email").unwrap(),
            "a@b.c"
        );
        // An empty value in the name claim has nothing to sign in as.
        assert_eq!(
            subject_name(&claims(",\"sub\":\"\""), "sub").err(),
            Some(Reason::NoSubject)
        );
        assert_eq!(
            subject_name(&claims(r#","sub":5"#), "sub").err(),
            Some(Reason::NoSubject)
        );
        assert_eq!(
            subject_name(&claims(""), "email").err(),
            Some(Reason::NoSubject)
        );
    }

    #[test]
    fn an_account_name_is_made_from_the_claim_value() {
        assert_eq!(account_name("Alice.B@Example.com"), "alice.b");
        assert_eq!(account_name("a b c@x"), "abc");
        // Nothing usable left after cleaning is no name at all: the caller
        // creates no account rather than naming one `user`.
        assert_eq!(account_name("!!!@x"), "");
    }
}
