// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Volume transfers. A transfer moves one volume, and the snapshots taken
//! from it, from the project that owns it to another.
//!
//! The owner offers an available volume and gets back a transfer id and a
//! secret. The secret is random, returned once with the offer and never
//! stored: only its SHA-256 goes in the record, the way a join token is
//! kept. Anyone who can name the volume can offer it; only someone who was
//! shown the secret can take it, so handing a volume over stays the owner's
//! decision even though the record itself holds no secret.
//!
//! A transfer is a record like the others, cloud-scoped (`meta.project` is
//! `None`) so both sides can see it, named by its id because nothing else
//! identifies it. It stays open for [`TRANSFER_LIFETIME`] and then expires.
//! Nothing waits for the expiry to arrive: an accept refuses a transfer it
//! finds past its window, and a pass over open transfers marks the expired
//! ones so a list tells the truth within one reconciler tick.

use chrono::{DateTime, Duration, Utc};
use pistack_types::Id;
use rand::RngCore;
use sha2::{Digest, Sha256};

use crate::quota::QuotaRequest;
use crate::types::{TransferPhase, KIND_VOLUME};
use crate::Transfer;

/// Id kind for a transfer. `xfer` is slab-private.
pub const KIND_TRANSFER: &str = "xfer";

/// How long an offered transfer stays open.
pub const TRANSFER_LIFETIME: Duration = Duration::hours(24);

/// The prefix of every transfer secret, so one can be recognised in a shell
/// history or a leaked log.
pub const SECRET_PREFIX: &str = "vxt_";

/// One process at a time may be between claiming a transfer and finishing
/// with it. The store has no compare-and-set that spans a read and a write,
/// and `Collection::modify` retries a revision clash rather than losing it,
/// so without this two callers could both find the same offer open and both
/// write themselves in as the taker.
pub(crate) fn claim_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    // Nothing is behind the lock, so a panic under it leaves nothing broken.
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// The transfer has been taken: the volume is no longer the giver's.
pub fn is_taken(t: &Transfer) -> bool {
    t.status.phase == TransferPhase::Accepted
}

/// Past its window. A record with no window — one written by a build that
/// did not set one — never expires on its own.
pub fn is_expired(t: &Transfer, now: DateTime<Utc>) -> bool {
    t.status.expires_at.is_some_and(|at| at <= now)
}

/// Closed to every caller: taken, already closed, or past its window.
pub fn is_closed(t: &Transfer, now: DateTime<Utc>) -> bool {
    t.status.phase != TransferPhase::Pending || is_expired(t, now)
}

/// What a project must have room for before the volume lands there: the
/// volume and every snapshot that follows it. Asked as one set, so a project
/// with room for the volume but not for its snapshots takes neither.
pub fn move_demand(size_gib: u64, snapshots: u64) -> Vec<QuotaRequest> {
    vec![
        QuotaRequest::volumes(1),
        QuotaRequest::gigabytes(size_gib),
        QuotaRequest::snapshots(snapshots),
    ]
}

/// The offer `t` is still open to, or `None` when it is not. The volume id
/// comes out of `spec.resource`, the stored hash out of the annotation.
pub fn open_offer(t: &Transfer, now: DateTime<Utc>) -> Option<(Id, String)> {
    if is_closed(t, now) {
        return None;
    }
    let volume = Id::parse(KIND_VOLUME, &t.spec.resource)?;
    let hash = t.meta.annotations.get(SECRET_HASH)?;
    Some((volume, hash.clone()))
}

/// The annotation holding the hash. Namespaced so a record of another kind
/// in the same table cannot collide with it.
pub const SECRET_HASH: &str = "pistack.io/transfer-secret";

/// A fresh one-time secret and the SHA-256 of it, lowercase hex, which is
/// all the record keeps. Secrets here are high-entropy, so a fast hash is
/// appropriate, unlike passwords.
pub(crate) fn new_secret() -> (String, String) {
    let mut buf = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut buf);
    let mut secret = String::with_capacity(SECRET_PREFIX.len() + 64);
    secret.push_str(SECRET_PREFIX);
    for b in buf {
        secret.push_str(&format!("{b:02x}"));
    }
    let hash = hash_secret(&secret);
    (secret, hash)
}

/// Hex SHA-256 of a secret, as [`new_secret`] stores it.
pub(crate) fn hash_secret(secret: &str) -> String {
    let digest = Sha256::digest(secret.as_bytes());
    let mut out = String::with_capacity(64);
    for b in digest {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

/// A transfer record with no name of its own: the id is the only thing that
/// identifies it, and a name would be another way to guess one.
pub(crate) fn record_name(id: &Id) -> String {
    id.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{TransferSpec, TransferStatus};
    use pistack_types::Meta;

    fn transfer(phase: TransferPhase, expires_in: Option<Duration>) -> Transfer {
        Transfer {
            meta: Meta::new(KIND_TRANSFER, "x", Some(Id::new("proj"))),
            spec: TransferSpec::default(),
            status: TransferStatus {
                phase,
                expires_at: expires_in.map(|d| Utc::now() + d),
                ..Default::default()
            },
        }
    }

    #[test]
    fn a_secret_is_random_and_only_its_hash_round_trips() {
        let (a, ha) = new_secret();
        let (b, hb) = new_secret();
        assert!(a.starts_with(SECRET_PREFIX), "{a}");
        assert_eq!(a.len(), SECRET_PREFIX.len() + 64);
        assert_ne!(a, b);
        assert_ne!(ha, hb);
        assert_eq!(ha, hash_secret(&a));
        assert_ne!(ha, hash_secret(&b));
        assert!(!a.contains(&ha));
    }

    #[test]
    fn a_demand_is_the_volume_plus_what_follows_it() {
        assert_eq!(
            move_demand(20, 3),
            vec![
                QuotaRequest::volumes(1),
                QuotaRequest::gigabytes(20),
                QuotaRequest::snapshots(3),
            ]
        );
    }

    #[test]
    fn only_an_open_transfer_is_offered() {
        let now = Utc::now();
        let t = transfer(TransferPhase::Pending, Some(Duration::hours(1)));
        assert!(!is_closed(&t, now));
        assert!(!is_expired(&t, now));

        let late = transfer(TransferPhase::Pending, Some(-Duration::seconds(1)));
        assert!(is_expired(&late, now));
        assert!(is_closed(&late, now));
        assert!(open_offer(&late, now).is_none());

        let taken = transfer(TransferPhase::Accepted, Some(Duration::hours(1)));
        assert!(is_taken(&taken));
        assert!(is_closed(&taken, now));

        // A record that predates the window is not expired by anything.
        let open = transfer(TransferPhase::Pending, None);
        assert!(!is_expired(&open, now));
    }
}
