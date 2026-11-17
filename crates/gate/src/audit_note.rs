// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! What a handler says about itself to the audit log, for the requests where
//! the caller alone does not tell the story.
//!
//! A write is recorded from the caller and the path. An administrator linking
//! someone else's account to an external identity is none of that: the record
//! must say whose account and which identity, which only the handler knows.
//! `Subject::default()` is the same empty subject the audit layer falls back to
//! for a request whose handler said nothing.

use crate::audit::Subject;

/// A subject carrying only notes. The caller on the request is the actor and is
/// read off it, so nothing here names a principal.
pub fn noted(notes: &[(&str, &str)]) -> Subject {
    Subject {
        notes: notes
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect(),
        ..Default::default()
    }
}
