// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Object bytes on more than one control-plane server.
//!
//! Bucket and object records replicate through the cluster store. The
//! bytes do not: a put lands on the disk of the server that took it, which
//! is the leader. This module gets them to the other servers.
//!
//! * **Peer endpoint.** `GET /silo/peer/blobs/{bucket-id}/{sha256}` serves
//!   bytes this server holds for an object record with that checksum. It
//!   only reads its own disk. It follows the same rule as `/cluster`: the
//!   caller must be a member, proved with the cluster's peer key. The
//!   call goes to the other server's TLS listener when it has one, as
//!   every call between servers does (`keep::peer::PeerNet`).
//! * **Fetch on miss.** A read that finds the record but not the bytes
//!   asks each other member of the cluster, leader first, streams the
//!   answer to a temp file, checks size and SHA-256, and renames it into
//!   place. See [`Client::open_object`].
//! * **Background pull.** [`run`] does the same for every object this
//!   server lacks, so the copies exist before a failover needs them, and
//!   on a follower removes bytes whose record is gone.
//!
//! A key can be overwritten, so a file at `{bucket}/{key}` is not proof
//! of holding the current bytes. Every put and every fetch writes a
//! marker, `{bucket}/.have/{sha256 of the key}`, holding the checksum of
//! the file and the key. A file whose marker names another checksum is
//! stale and is fetched again. A file with no marker predates this module
//! and was written by the only server there was, so it is trusted.
//!
//! * **Copy before the answer.** A put is answered only after one other
//!   server holds the bytes: the server that took it calls
//!   `POST /silo/peer/pull/{object-id}/{sha256}` on each peer in turn and
//!   the first one that answers has fetched and checked them. See
//!   [`Peers::push`]. `[modules.silo] put_copies = 1` turns the wait off.
//!
//! Multipart staging is copied by [`crate::staging`].

mod connect_timeout;
mod hex;
mod marker;
mod marker_dir;
mod marker_path;
mod member;
mod older_than;
mod own_member_id;
mod peers;
mod prune_grace;
mod prune_orphans;
mod push_base;
mod push_wait;
mod put_copies;
mod read_marker;
mod read_marker_file;
mod record_wait;
mod remove_marker;
mod replicate_interval_secs;
mod run;
mod source;
mod stall;
mod sync_once;
mod sync_report;
#[cfg(test)]
mod tests;
mod write_marker;
pub(crate) use connect_timeout::CONNECT_TIMEOUT;
pub use peers::Peers;
pub use prune_grace::PRUNE_GRACE;
pub(crate) use push_wait::push_wait;
pub use put_copies::PUT_COPIES;
pub(crate) use read_marker::read_marker;
pub use record_wait::RECORD_WAIT;
pub(crate) use remove_marker::remove_marker;
pub use replicate_interval_secs::REPLICATE_INTERVAL_SECS;
pub use run::run;
pub use sync_once::sync_once;
pub use sync_report::SyncReport;
pub(crate) use write_marker::write_marker;
