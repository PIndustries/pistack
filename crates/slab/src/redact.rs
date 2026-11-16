// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Stripping the one field of a transfer record that must not be answered
//! with: the annotation holding the SHA-256 of the one-time secret.
//!
//! The hash lives in the record because the accept path needs it and the
//! store has no other place to keep it. Every HTTP answer is written by hand
//! out of the record, so every one of them has to drop it — including the
//! admin answers, which see records across projects.

use crate::transfer::SECRET_HASH;
use crate::Transfer;

/// `t` with the secret hash annotation gone. Nothing else about the record
/// changes, so the id, the phase and the volume it names still answer, and
/// the stored record — the one `accept` checks a secret against — is
/// untouched.
pub(crate) fn without_secret_hash(t: Transfer) -> Transfer {
    let mut t = t;
    t.meta.annotations.remove(SECRET_HASH);
    t
}
