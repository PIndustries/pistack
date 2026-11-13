// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Re-encrypting every sealed payload under a new root key.
//!
//! Software backend only, and offline: `pistack keep rotate-root-key` runs
//! this with the server stopped. A running server holds the root key in
//! memory and would go on sealing with the old one.
//!
//! The steps, each one store write that either happened or did not:
//!
//! 1. The new key is saved beside the old one, under
//!    [`META_ROOT_KEY_NEXT`]. From here until the last step the store is
//!    "in rotation" and a server refuses to start on it.
//! 2. Each secret that still opens only with the old key is opened with it
//!    and sealed again with the new one. A secret that already opens with
//!    the new key is left alone, which is what makes a second run pick up
//!    where the first one stopped.
//! 3. Every secret is opened once more with the new key. Nothing is thrown
//!    away before this passes.
//! 4. In one transaction the new key becomes [`META_ROOT_KEY`] and
//!    [`META_ROOT_KEY_NEXT`] is deleted.
//!
//! The old key is in the store until step 4, so stopping anywhere before
//! it loses nothing: run the command again. Which key sealed a payload is
//! found by trying: AES-GCM authenticates, so the wrong key fails cleanly.
//!
//! A cluster keeps one copy of the store per server, and this changes only
//! the copy it is run on. Every server must end up with the same key, so
//! the key is made once, written to a file, and the command is run on each
//! server with that file.

use base64::{engine::general_purpose::STANDARD as B64, Engine};
use chrono::{DateTime, Utc};
use pistack_store::Store;
use pistack_types::Id;
use subtle::ConstantTimeEq;

use crate::backend::{root_key_state, RootKeyState};
use crate::crypto::{RootKey, META_ROOT_KEY};
use crate::error::{KeepError, Result};
use crate::types::StoredSecret;

/// Store meta key holding the key a rotation is moving to. Present only
/// while one is unfinished.
pub const META_ROOT_KEY_NEXT: &str = "keep/root_key_next";
/// Store meta key holding when the root key was last rotated.
pub const META_ROOT_KEY_ROTATED_AT: &str = "keep/root_key_rotated_at";

/// What a run did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rekeyed {
    /// An earlier run had started this rotation.
    pub resumed: bool,
    /// Payloads sealed again by this run.
    pub resealed: usize,
    /// Payloads an earlier run had already moved to the new key.
    pub already: usize,
    /// Secrets with no payload.
    pub empty: usize,
}

fn err(msg: impl Into<String>) -> KeepError {
    KeepError::Backend(msg.into())
}

fn encode(key: &RootKey) -> String {
    B64.encode(key.bytes())
}

/// Read a root key as the store and the key file hold it: base64 of 32
/// bytes.
pub fn decode(b64: &str) -> Result<RootKey> {
    let raw = B64
        .decode(b64.trim())
        .map_err(|e| KeepError::Crypto(format!("root key is not base64: {e}")))?;
    let bytes: [u8; 32] = raw
        .try_into()
        .map_err(|_| KeepError::Crypto("root key must be 32 bytes".into()))?;
    Ok(RootKey::from_bytes(bytes))
}

/// A new random key in the form [`decode`] reads.
pub fn generate_encoded() -> String {
    encode(&RootKey::generate())
}

fn same(a: &RootKey, b: &RootKey) -> bool {
    bool::from(a.bytes().ct_eq(b.bytes()))
}

/// Is a rotation unfinished on this store?
pub fn in_progress(store: &Store) -> Result<bool> {
    Ok(store.get_meta::<String>(META_ROOT_KEY_NEXT)?.is_some())
}

/// When the root key was last rotated, if it ever was.
pub fn rotated_at(store: &Store) -> Result<Option<DateTime<Utc>>> {
    Ok(store.get_meta::<DateTime<Utc>>(META_ROOT_KEY_ROTATED_AT)?)
}

/// Move every sealed payload to a new root key, or finish a rotation an
/// earlier run started. `new_key` is the key to move to; without one a
/// random key is made. A rotation already started keeps the key it
/// started with, and a different `new_key` is refused.
pub fn rotate_root_key(store: &Store, new_key: Option<RootKey>) -> Result<Rekeyed> {
    rotate_inner(store, new_key, None)
}

/// [`rotate_root_key`] that stops with an error once `stop_after` payloads
/// have been sealed again: what a crash at that point leaves behind.
fn rotate_inner(
    store: &Store,
    new_key: Option<RootKey>,
    stop_after: Option<usize>,
) -> Result<Rekeyed> {
    match root_key_state(store)? {
        RootKeyState::Software => {}
        RootKeyState::Empty => {
            return Err(err("this store has no root key yet; nothing to rotate"))
        }
        RootKeyState::Wrapped(_) | RootKeyState::Both(_) => {
            return Err(err(
                "the root key of this store is wrapped by Vault Transit. Re-encrypting under \
                 a new root key is only done for the software backend; rotate the Transit key \
                 in Vault and run `pistack keep rewrap`",
            ))
        }
    }
    let old = decode(
        &store
            .get_meta::<String>(META_ROOT_KEY)?
            .ok_or_else(|| err("the root key went missing"))?,
    )?;

    let (next, resumed) = match store.get_meta::<String>(META_ROOT_KEY_NEXT)? {
        Some(saved) => {
            let saved = decode(&saved)?;
            if new_key.as_ref().is_some_and(|k| !same(k, &saved)) {
                return Err(err(
                    "a rotation is unfinished on this store and it was started with another \
                     key than the one given; run again with the key file it was started \
                     with, or with none to finish with the key the store holds",
                ));
            }
            (saved, true)
        }
        None => {
            let next = new_key.unwrap_or_else(RootKey::generate);
            if same(&next, &old) {
                return Err(err("the new root key is the one already in use"));
            }
            store.put_meta(META_ROOT_KEY_NEXT, &encode(&next))?;
            (next, false)
        }
    };

    let col = store.collection::<StoredSecret>();
    let mut out = Rekeyed {
        resumed,
        resealed: 0,
        already: 0,
        empty: 0,
    };
    for s in col.list(None)? {
        let id = s.meta.id.clone();
        let aad = id.to_string();
        let Some(sealed) = &s.status.sealed else {
            out.empty += 1;
            continue;
        };
        if next.open(sealed, aad.as_bytes()).is_ok() {
            out.already += 1;
            continue;
        }
        let payload = old.open(sealed, aad.as_bytes()).map_err(|_| {
            unreadable(&id, "neither the current root key nor the new one opens it")
        })?;
        if stop_after.is_some_and(|n| out.resealed >= n) {
            return Err(err("interrupted"));
        }
        let fresh = next.seal(&payload, aad.as_bytes())?;
        col.modify(&id, |cur| {
            cur.status.sealed = Some(fresh.clone());
            Ok(())
        })?;
        out.resealed += 1;
    }

    // Nothing of the old key is dropped until every payload is known to
    // open with the new one.
    for s in col.list(None)? {
        if let Some(sealed) = &s.status.sealed {
            next.open(sealed, s.meta.id.to_string().as_bytes())
                .map_err(|_| unreadable(&s.meta.id, "the new root key does not open it"))?;
        }
    }
    store.put_and_delete_meta(META_ROOT_KEY, &encode(&next), META_ROOT_KEY_NEXT)?;
    store.put_meta(META_ROOT_KEY_ROTATED_AT, &Utc::now())?;
    Ok(out)
}

fn unreadable(id: &Id, why: &str) -> KeepError {
    err(format!(
        "secret {id}: {why}. Nothing was dropped: the store still holds the current root key \
         and the rotation can be run again once this secret is dealt with"
    ))
}

/// The error a server gives when started on a store in rotation.
pub(crate) fn refuse_start() -> KeepError {
    err(format!(
        "a root key rotation is unfinished on this store ({META_ROOT_KEY_NEXT} is set): some \
         secrets are sealed with the new key and some with the old. Stop the server and run \
         `pistack keep rotate-root-key` again to finish it"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::KeyBackend;
    use crate::types::{Secret, SecretSpec};
    use crate::Client;

    /// A software store as a running cloud has it: the CA with its two
    /// keys, a server certificate with its key, an access-key-shaped
    /// secret and `n` plain ones.
    fn cloud(store: &Store, n: usize) -> Vec<(Id, Vec<u8>)> {
        let c = Client::open_with(store, &KeyBackend::Software).unwrap();
        c.issue_server_cert("pistack", &["10.0.0.11".to_owned()])
            .unwrap();
        (0..n)
            .map(|i| {
                let payload = format!("payload-{i}").into_bytes();
                let s = c
                    .create_secret(
                        Secret::new(format!("s{i}"), None, SecretSpec::default()),
                        &payload,
                    )
                    .unwrap();
                (s.meta.id, payload)
            })
            .collect()
    }

    /// Open the store as a server would and read everything back.
    fn everything_opens(store: &Store, secrets: &[(Id, Vec<u8>)], ca_pem: &str) {
        let c = Client::open_with(store, &KeyBackend::Software).unwrap();
        for (id, payload) in secrets {
            assert_eq!(&c.get_secret_payload(id).unwrap(), payload);
        }
        // The CA keys were sealed too: it loads and signs.
        assert_eq!(c.ca_bundle_pem(), ca_pem);
        c.issue_client_cert("n1").unwrap();
        for cert in c.list_certificates(None).unwrap() {
            if cert.status.key_secret.is_some() {
                assert!(c
                    .certificate_key_pem(&cert)
                    .unwrap()
                    .contains("PRIVATE KEY"));
            }
        }
    }

    fn key_in(store: &Store) -> String {
        store.get_meta::<String>(META_ROOT_KEY).unwrap().unwrap()
    }

    #[test]
    fn every_payload_moves_to_the_new_key() {
        let store = Store::in_memory().unwrap();
        let secrets = cloud(&store, 5);
        let ca = Client::open_with(&store, &KeyBackend::Software)
            .unwrap()
            .ca_bundle_pem();
        let before = key_in(&store);
        let sealed = store.collection::<StoredSecret>().list(None).unwrap().len();
        assert!(rotated_at(&store).unwrap().is_none());

        let r = rotate_root_key(&store, None).unwrap();
        assert!(!r.resumed);
        // The five secrets, two CA keys and the server certificate's key.
        assert_eq!((r.resealed, r.already, r.empty), (sealed, 0, 0));
        assert_eq!(sealed, 8);
        assert_ne!(key_in(&store), before);
        assert!(!in_progress(&store).unwrap());
        assert!(rotated_at(&store).unwrap().is_some());
        everything_opens(&store, &secrets, &ca);

        // The old key opens nothing any more.
        let old = decode(&before).unwrap();
        for s in store.collection::<StoredSecret>().list(None).unwrap() {
            let sealed = s.status.sealed.unwrap();
            assert!(old.open(&sealed, s.meta.id.to_string().as_bytes()).is_err());
        }
    }

    #[test]
    fn a_rotation_interrupted_halfway_resumes_and_every_secret_still_decrypts() {
        let store = Store::in_memory().unwrap();
        let secrets = cloud(&store, 6);
        let ca = Client::open_with(&store, &KeyBackend::Software)
            .unwrap()
            .ca_bundle_pem();
        let before = key_in(&store);
        let total = store.collection::<StoredSecret>().list(None).unwrap().len();

        // Stopped after four of the nine payloads.
        let stopped = rotate_inner(&store, None, Some(4)).unwrap_err();
        assert!(stopped.to_string().contains("interrupted"), "{stopped}");
        assert!(in_progress(&store).unwrap());
        // The current key is still the old one, and both keys are needed:
        // a server must not start here.
        assert_eq!(key_in(&store), before);
        let refused = KeyBackend::Software.load_existing(&store).unwrap_err();
        assert!(refused.to_string().contains("rotate-root-key"), "{refused}");
        assert!(Client::open_with(&store, &KeyBackend::Software).is_err());
        let old = decode(&before).unwrap();
        let next = decode(
            &store
                .get_meta::<String>(META_ROOT_KEY_NEXT)
                .unwrap()
                .unwrap(),
        )
        .unwrap();
        let mut by_old = 0;
        let mut by_next = 0;
        for s in store.collection::<StoredSecret>().list(None).unwrap() {
            let aad = s.meta.id.to_string();
            let sealed = s.status.sealed.unwrap();
            by_old += usize::from(old.open(&sealed, aad.as_bytes()).is_ok());
            by_next += usize::from(next.open(&sealed, aad.as_bytes()).is_ok());
        }
        assert_eq!((by_next, by_old), (4, total - 4));

        // Interrupted again, further on.
        assert!(rotate_inner(&store, None, Some(2)).is_err());

        // Another key is refused: the rotation finishes with the one it
        // started with.
        let other = rotate_root_key(&store, Some(RootKey::generate())).unwrap_err();
        assert!(other.to_string().contains("another"), "{other}");

        let r = rotate_root_key(&store, None).unwrap();
        assert!(r.resumed);
        assert_eq!((r.already, r.resealed), (6, total - 6));
        assert!(!in_progress(&store).unwrap());
        assert_eq!(key_in(&store), encode(&next));
        everything_opens(&store, &secrets, &ca);
    }

    #[test]
    fn the_same_key_file_gives_every_server_the_same_key() {
        let key = generate_encoded();
        let mut ends = Vec::new();
        for _ in 0..2 {
            let store = Store::in_memory().unwrap();
            let secrets = cloud(&store, 2);
            let ca = Client::open_with(&store, &KeyBackend::Software)
                .unwrap()
                .ca_bundle_pem();
            rotate_root_key(&store, Some(decode(&key).unwrap())).unwrap();
            everything_opens(&store, &secrets, &ca);
            ends.push(key_in(&store));
        }
        assert_eq!(ends, [key.clone(), key.clone()]);

        // Running it again with the key now in use is refused, not a
        // silent second rotation.
        let store = Store::in_memory().unwrap();
        cloud(&store, 1);
        rotate_root_key(&store, Some(decode(&key).unwrap())).unwrap();
        let again = rotate_root_key(&store, Some(decode(&key).unwrap())).unwrap_err();
        assert!(again.to_string().contains("already in use"), "{again}");
    }

    #[test]
    fn a_payload_no_key_opens_stops_the_rotation_before_anything_is_dropped() {
        let store = Store::in_memory().unwrap();
        let secrets = cloud(&store, 3);
        let before = key_in(&store);
        // Sealed with a key that is neither the old one nor the new one.
        let stray = RootKey::generate();
        let id = secrets[1].0.clone();
        store
            .collection::<StoredSecret>()
            .modify(&id, |s| {
                s.status.sealed = Some(stray.seal(b"lost", id.to_string().as_bytes()).unwrap());
                Ok(())
            })
            .unwrap();
        let e = rotate_root_key(&store, None).unwrap_err();
        assert!(e.to_string().contains(&id.to_string()), "{e}");
        assert_eq!(key_in(&store), before);
        assert!(in_progress(&store).unwrap());
    }

    #[test]
    fn stores_that_cannot_be_rotated_say_why() {
        let empty = Store::in_memory().unwrap();
        let e = rotate_root_key(&empty, None).unwrap_err();
        assert!(e.to_string().contains("no root key"), "{e}");
        assert!(decode("not base64!").is_err());
        assert!(decode(&B64.encode([0u8; 16])).is_err());
        assert!(decode(&format!("{}\n", generate_encoded())).is_ok());
    }
}
