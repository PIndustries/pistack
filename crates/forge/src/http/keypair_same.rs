// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::app_state::AppState;
use super::load_keypair::load_keypair;
use super::scope::Scope;

/// The public key of a pair named in the request, in the caller's project.
/// A machine holds the key itself from here on; the name is not kept.
/// Whether two spellings name the same key pair of this project. Equal
/// strings are enough; otherwise both must resolve, to one resource.
pub(crate) fn keypair_same(s: &AppState, scope: &Scope, a: &str, b: &str) -> bool {
    a == b
        || match (load_keypair(s, scope, a), load_keypair(s, scope, b)) {
            (Ok(x), Ok(y)) => x.meta.id == y.meta.id,
            _ => false,
        }
}
