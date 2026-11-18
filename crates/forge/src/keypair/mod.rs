// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! SSH keys in the formats a guest already takes.
//!
//! A key is stored as the OpenSSH line the guest receives, so what the API
//! keeps is byte for byte what cloud-init writes. `ed25519`, `ecdsa` and
//! `rsa` are read from their wire blobs; `dsa` and anything malformed is
//! refused. An `rsa` key must be 2048 bits or larger.
//!
//! An `ed25519` pair can also be made here. The private half leaves through
//! the create response in OpenSSH's private key format and is not stored.

mod b64;
mod b64_nopad;
mod bad;
mod curve_of;
mod ecdsa_p256;
mod ecdsa_p384;
mod ecdsa_p521;
mod ed25519_priv_len;
mod ed25519_pub_len;
mod ed25519_seed_len;
mod ed25519_type;
mod field_bytes_of;
mod fingerprint;
mod generate_ed25519;
mod generated;
mod min_rsa_bits;
mod mpint;
mod parse;
mod parse_blob;
mod public_key;
mod random;
mod rsa;
mod ssh_string;
mod string_field;
#[cfg(test)]
mod tests;
pub use ecdsa_p256::ECDSA_P256;
pub use ecdsa_p384::ECDSA_P384;
pub use ecdsa_p521::ECDSA_P521;
#[cfg(test)]
pub(crate) use ed25519_priv_len::ED25519_PRIV_LEN;
#[cfg(test)]
pub(crate) use ed25519_seed_len::ED25519_SEED_LEN;
pub use ed25519_type::ED25519_TYPE;
#[cfg(test)]
pub(crate) use fingerprint::fingerprint;
pub use generate_ed25519::generate_ed25519;
pub use generated::Generated;
pub use parse::parse;
pub use public_key::PublicKey;
pub use rsa::RSA;
