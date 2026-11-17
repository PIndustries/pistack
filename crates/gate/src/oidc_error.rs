// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The one answer an OIDC sign-in gives when it does not sign anyone in, and
//! the reason it keeps for the audit record.
//!
//! Every failure — a token that will not verify, an issuer or audience that
//! does not match, an unknown user, a disabled account, a provider that is
//! not there — is answered with the same message at the same status. Only the
//! audit note says which check failed, so a caller cannot probe the cloud for
//! what it holds by reading the refusals.

use crate::audit::Subject;
use crate::types::IdentityProvider;
use pistack_types::ApiError;

/// The reason a sign-in was refused. Written to the audit note, never to the
/// HTTP answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reason {
    /// No provider with that name, or one that could not be loaded.
    UnknownProvider,
    /// A provider stored with a bad issuer URL (leftover from before).
    BadIssuer,
    /// Discovery or the key set could not be fetched.
    KeysUnavailable,
    /// The token is malformed, its algorithm is not allowed, or its signature
    /// does not check out.
    Signature,
    /// The signature is fine but the issuer, audience or times are not.
    Claims,
    /// The `email_verified` check failed.
    EmailNotVerified,
    /// The token carries no usable value in the configured name claim.
    NoSubject,
    /// The `sub` the token carries matches no account and the provider does not
    /// create users.
    UnknownUser,
    /// The matching account is disabled.
    Disabled,
    /// The bound account is a service account.
    NotAUser,
    /// No legal account name could be made for a new account, or the store would
    /// not create it.
    NameTaken,
    /// The store could not be consulted. Never named to the caller.
    StoreFault,
}

impl Reason {
    /// The short word put in the audit note.
    pub fn as_str(self) -> &'static str {
        match self {
            Reason::UnknownProvider => "unknown provider",
            Reason::BadIssuer => "bad issuer",
            Reason::KeysUnavailable => "keys unavailable",
            Reason::Signature => "signature",
            Reason::Claims => "claims",
            Reason::EmailNotVerified => "email not verified",
            Reason::NoSubject => "no subject",
            Reason::UnknownUser => "unknown user",
            Reason::Disabled => "disabled",
            Reason::NotAUser => "not a user",
            Reason::NameTaken => "name taken",
            Reason::StoreFault => "store fault",
        }
    }
}

/// The refusal for a failed OIDC sign-in. Names nothing about what failed and
/// carries no principal id.
pub fn refused() -> ApiError {
    ApiError::unauthorized("the identity token was not accepted")
}

/// Record who the sign-in was for and why it ended, on the audit subject the
/// login handler hands back. The provider is named whenever one was chosen,
/// even on a refusal that never got as far as a user.
pub fn note(
    subject: &mut Subject,
    provider: Option<&IdentityProvider>,
    name: Option<&str>,
    principal_id: Option<pistack_types::Id>,
    reason: Option<Reason>,
) {
    if let Some(p) = provider {
        subject.notes.insert("provider".into(), p.meta.name.clone());
    }
    if let Some(n) = name {
        subject.principal = Some(n.to_owned());
    }
    if principal_id.is_some() {
        subject.principal_id = principal_id;
    }
    if let Some(r) = reason {
        subject.notes.insert("reason".into(), r.as_str().to_owned());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_refusal_is_the_same_answer() {
        let a = refused();
        assert_eq!(a.message, "the identity token was not accepted");
        assert_eq!(a.status().as_u16(), 401);
    }

    #[test]
    fn a_note_names_the_provider_and_the_reason() {
        use super::super::types::{IdentityProvider, IdentityProviderSpec, IdentityProviderStatus};
        use pistack_types::Meta;
        let provider = IdentityProvider {
            meta: Meta::new(crate::types::KIND_PROVIDER, "corp", None),
            spec: IdentityProviderSpec::default(),
            status: IdentityProviderStatus::default(),
        };
        let mut subject = Subject::default();
        note(
            &mut subject,
            Some(&provider),
            Some("alice@example.com"),
            None,
            Some(Reason::Disabled),
        );
        assert_eq!(
            subject.notes.get("provider").map(String::as_str),
            Some("corp")
        );
        assert_eq!(
            subject.notes.get("reason").map(String::as_str),
            Some("disabled")
        );
        assert_eq!(subject.principal.as_deref(), Some("alice@example.com"));
    }
}
