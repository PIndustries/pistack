// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The OpenID Connect ID token claims, as little as sign-in needs from them.
//!
//! The signature and the registered claims (`iss`, `aud`, `exp`, `nbf`) are
//! checked by the JWT decoder, which parses the payload into this type. What
//! is left over — the claim that names the user, the optional groups claim, and
//! the `email_verified` flag — is read out of the parsed payload here. The
//! claims are kept as the decoded JSON object rather than split into named
//! fields: the claims gate cares about are named by the provider record, so a
//! flat `Map` with typed accessors is the honest shape and avoids a declared
//! field silently colliding with the flattened rest.

use serde_json::Value;

/// The whole decoded payload of an ID token.
#[derive(Clone, Debug)]
pub struct IdTokenClaims {
    claims: serde_json::Map<String, Value>,
}

impl<'de> serde::Deserialize<'de> for IdTokenClaims {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::Error as _;
        let obj = serde_json::Map::<String, Value>::deserialize(deserializer)?;
        // The registered claims gate relies on are checked by the decoder, but
        // it only errors if they are *wrong*, not absent. Insist on the ones a
        // token without them must not be signed in with. `sub` is among them:
        // it is the only thing that says which account the token is for, and a
        // token that does not carry one is not an ID token.
        for required in ["iss", "aud", "exp", "sub"] {
            if !obj.contains_key(required) {
                return Err(D::Error::missing_field(required));
            }
        }
        Ok(IdTokenClaims { claims: obj })
    }
}

impl IdTokenClaims {
    /// The `sub` claim. Empty and non-string values are not a subject: the
    /// pair `(provider, sub)` is what an account is bound to, so a token that
    /// cannot produce one is refused rather than matched loosely.
    pub fn subject(&self) -> Option<&str> {
        self.claim_str("sub").filter(|s| !s.is_empty())
    }

    /// The value of `claim`, when the token carries it.
    pub fn claim(&self, claim: &str) -> Option<&Value> {
        self.claims.get(claim)
    }

    /// The value of `claim` as a string. A non-string value is not a name.
    pub fn claim_str(&self, claim: &str) -> Option<&str> {
        self.claim(claim).and_then(|v| v.as_str())
    }

    /// `email_verified` as a real boolean. A provider that stores it as the
    /// string `"true"` is treated as verified; anything else is not.
    pub fn email_verified(&self) -> Option<bool> {
        match self.claim("email_verified") {
            Some(v) => match v.as_bool() {
                Some(b) => Some(b),
                None => v.as_str().map(|s| s.eq_ignore_ascii_case("true")),
            },
            None => None,
        }
    }

    /// The groups named by `claim`, when it holds an array of strings. A single
    /// string or any other shape is not a group list and yields nothing.
    pub fn groups(&self, claim: &str) -> Vec<String> {
        match self.claim(claim) {
            Some(Value::Array(items)) => items
                .iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect(),
            _ => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claims(json: &str) -> IdTokenClaims {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn a_token_without_the_registered_claims_is_not_parsed() {
        assert!(serde_json::from_str::<IdTokenClaims>(r#"{"sub":"s","aud":"c","exp":1}"#).is_err());
        assert!(serde_json::from_str::<IdTokenClaims>(r#"{"sub":"s","iss":"i","exp":1}"#).is_err());
        assert!(
            serde_json::from_str::<IdTokenClaims>(r#"{"sub":"s","iss":"i","aud":"c"}"#).is_err()
        );
        // `sub` is required like the other three: the whole thing a sign-in
        // matches an account on, so a token without it names nobody.
        assert!(serde_json::from_str::<IdTokenClaims>(r#"{"iss":"i","aud":"c","exp":1}"#).is_err());
    }

    #[test]
    fn the_subject_is_the_sub_claim_and_only_that() {
        let c = claims(r#"{"iss":"i","aud":"c","exp":1,"sub":"user-7","email":"a@b.c"}"#);
        assert_eq!(c.subject(), Some("user-7"));
        // An empty or non-string `sub` is not a subject: it cannot be the half
        // of a pair an account is bound to.
        let blank = claims(r#"{"iss":"i","aud":"c","exp":1,"sub":""}"#);
        assert_eq!(blank.subject(), None);
        let number = claims(r#"{"iss":"i","aud":"c","exp":1,"sub":7}"#);
        assert_eq!(number.subject(), None);
    }

    #[test]
    fn every_claim_is_reachable_by_name() {
        let c = claims(
            r#"{"iss":"i","sub":"s","aud":["a","b"],"exp":1,"email":"a@b.c","groups":["ops","web"]}"#,
        );
        assert_eq!(c.claim_str("email"), Some("a@b.c"));
        assert_eq!(c.claim_str("absent"), None);
        assert!(c.claim("iss").is_some());
        assert_eq!(c.groups("groups"), ["ops".to_string(), "web".to_string()]);
        // A scalar, a number and an absent claim are all "no groups".
        assert!(c.groups("email").is_empty());
        assert!(c.groups("exp").is_empty());
        assert!(c.groups("absent").is_empty());
    }

    #[test]
    fn email_verified_reads_a_bool_a_string_and_absent() {
        assert_eq!(
            claims(r#"{"iss":"i","sub":"s","aud":"c","exp":1,"email_verified":true}"#)
                .email_verified(),
            Some(true)
        );
        assert_eq!(
            claims(r#"{"iss":"i","sub":"s","aud":"c","exp":1,"email_verified":"TRUE"}"#)
                .email_verified(),
            Some(true)
        );
        assert_eq!(
            claims(r#"{"iss":"i","sub":"s","aud":"c","exp":1,"email_verified":"no"}"#)
                .email_verified(),
            Some(false)
        );
        assert_eq!(
            claims(r#"{"iss":"i","sub":"s","aud":"c","exp":1,"email_verified":false}"#)
                .email_verified(),
            Some(false)
        );
        assert_eq!(
            claims(r#"{"iss":"i","sub":"s","aud":"c","exp":1}"#).email_verified(),
            None
        );
    }

    #[test]
    fn unknown_claims_are_kept_and_read_back() {
        let c = claims(r#"{"iss":"i","sub":"s","aud":"c","exp":1,"name":"A","picture":"u"}"#);
        assert_eq!(c.claim_str("name"), Some("A"));
        assert!(c.claim("picture").is_some());
    }
}
