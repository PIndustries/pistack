// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use base64::Engine as _;
use ring::signature::KeyPair as _;

use super::ed25519_priv_len::ED25519_PRIV_LEN;
use super::ed25519_seed_len::ED25519_SEED_LEN;
use super::ed25519_type::ED25519_TYPE;
use super::fingerprint::fingerprint;
use super::generated::Generated;
use super::public_key::PublicKey;
use super::random::random;
use super::ssh_string::ssh_string;

/// An `ed25519` key pair. `ring` makes the seed and derives the public half;
/// the private key is then laid out the way `ssh-keygen` writes one.
pub fn generate_ed25519(comment: &str) -> Generated {
    let pkcs8 = ring::signature::Ed25519KeyPair::generate_pkcs8(&ring::rand::SystemRandom::new())
        .expect("ed25519 pkcs8 from a fresh key");
    let pair = ring::signature::Ed25519KeyPair::from_pkcs8(pkcs8.as_ref())
        .expect("the pkcs8 just generated");
    let public_bytes = pair.public_key().as_ref().to_vec();
    // The seed is the last 32 bytes of the wrapper and nothing else.
    let seed = pkcs8.as_ref()[pkcs8.as_ref().len() - ED25519_SEED_LEN..].to_vec();

    let mut blob = Vec::new();
    blob.extend_from_slice(&ssh_string(ED25519_TYPE.as_bytes()));
    blob.extend_from_slice(&ssh_string(&public_bytes));
    let public = PublicKey {
        key_type: ED25519_TYPE.to_owned(),
        line: format!(
            "{ED25519_TYPE} {}{}",
            base64::engine::general_purpose::STANDARD.encode(&blob),
            if comment.is_empty() {
                String::new()
            } else {
                format!(" {comment}")
            }
        ),
        fingerprint: fingerprint(&blob),
    };

    // What an unencrypted single-key file holds after its length: two
    // identical check ints, the key type, the seed, the seed with its public
    // half, then the comment.
    let privkey = [seed.as_slice(), public_bytes.as_slice()].concat();
    debug_assert_eq!(privkey.len(), ED25519_PRIV_LEN);
    let check = random(4);
    let mut privbody = Vec::new();
    privbody.extend_from_slice(&check);
    privbody.extend_from_slice(&check);
    privbody.extend_from_slice(&ssh_string(ED25519_TYPE.as_bytes()));
    privbody.extend_from_slice(&ssh_string(&seed));
    privbody.extend_from_slice(&ssh_string(&privkey));
    privbody.extend_from_slice(&ssh_string(comment.as_bytes()));
    // Padding counts up from 1 to the next multiple of the cipher block, and
    // the length covers the body and its padding.
    let pad = 8 - (privbody.len() % 8) % 8;
    privbody.extend((1..=pad).map(|i| i as u8));
    let privblob = privbody;

    let mut body = Vec::new();
    body.extend_from_slice(b"openssh-key-v1\0");
    body.extend_from_slice(&ssh_string(b"none"));
    body.extend_from_slice(&ssh_string(b"none"));
    body.extend_from_slice(&ssh_string(b""));
    body.extend(&1u32.to_be_bytes());
    body.extend_from_slice(&ssh_string(&blob));
    body.extend_from_slice(&ssh_string(&privblob));

    // The body is padded so its base64 needs no `=`; the public line is
    // written the way `ssh-keygen` writes it, padding and all.
    let mut pem = String::from("-----BEGIN OPENSSH PRIVATE KEY-----\n");
    let encoded = base64::engine::general_purpose::STANDARD.encode(&body);
    for chunk in encoded.as_bytes().chunks(70) {
        pem.push_str(&String::from_utf8_lossy(chunk));
        pem.push('\n');
    }
    pem.push_str("-----END OPENSSH PRIVATE KEY-----\n");
    Generated {
        public,
        private_openssh: pem,
    }
}
