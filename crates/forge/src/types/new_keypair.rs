// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::{Id, Meta, Resource};

use super::keypair::Keypair;
use super::keypair_kind::KEYPAIR_KIND;
use super::keypair_spec::KeypairSpec;
use super::keypair_status::KeypairStatus;

pub fn new_keypair(
    name: impl Into<String>,
    project: Option<Id>,
    key: crate::keypair::PublicKey,
) -> Keypair {
    Keypair(Resource {
        meta: Meta::new(KEYPAIR_KIND, name, project),
        spec: KeypairSpec {
            public_key: key.line,
            key_type: key.key_type,
            fingerprint: key.fingerprint,
        },
        status: KeypairStatus {},
    })
}
