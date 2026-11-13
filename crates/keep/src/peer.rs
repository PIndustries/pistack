// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! How one control-plane server proves to another that it is a member.
//!
//! The key is derived from the token signing secret, which is in every
//! member's store and in every snapshot and nowhere else. A server has it
//! from the moment it holds the cluster's state and needs nothing handed
//! to it. The cluster routes and the object-bytes route both check it.

use hmac::Mac;
use pistack_store::Store;
use subtle::ConstantTimeEq;

/// Header a member sends on every call to another server: the peer key.
pub const PEER_HEADER: &str = "x-pistack-cluster";

/// The key members prove themselves with. `None` before the identity
/// module has made the signing secret.
pub fn peer_key(store: &Store) -> Option<String> {
    let secret = store
        .get_meta::<String>(gate::token::META_JWT_SECRET)
        .ok()??;
    let mut mac = hmac::Hmac::<sha2::Sha256>::new_from_slice(secret.as_bytes()).ok()?;
    mac.update(b"pistack cluster peer v1");
    let digest = mac.finalize().into_bytes();
    Some(digest.iter().map(|b| format!("{b:02x}")).collect())
}

/// Whether `offered` is this cluster's peer key. Compared in constant time.
pub fn is_member(store: &Store, offered: &[u8]) -> bool {
    match peer_key(store) {
        Some(key) => bool::from(key.as_bytes().ct_eq(offered)),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_key_of_this_store_is_a_member() {
        let store = Store::in_memory().unwrap();
        assert!(peer_key(&store).is_none());
        assert!(!is_member(&store, b""));
        store
            .put_meta(gate::token::META_JWT_SECRET, &"c2VjcmV0".to_owned())
            .unwrap();
        let key = peer_key(&store).unwrap();
        assert_eq!(key.len(), 64);
        assert!(is_member(&store, key.as_bytes()));
        assert!(!is_member(&store, b"guess"));

        let other = Store::in_memory().unwrap();
        other
            .put_meta(gate::token::META_JWT_SECRET, &"b3RoZXI=".to_owned())
            .unwrap();
        assert!(!is_member(&other, key.as_bytes()));
    }
}
