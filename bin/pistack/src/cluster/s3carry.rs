// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! What the write gate has to know about the S3 API mounted beside it.
//!
//! An S3 client cannot be told to ask for a detour. It has no header it can
//! put on a request — nothing an aws cli, a boto3 or an s3cmd will send reads
//! as `x-pistack-forward: leader` — and a JSON 409 tells a client that reads
//! XML nothing. So a write that lands on a follower under the S3 mount point
//! is carried whether or not anyone asked.
//!
//! Which part of the address space that is does not come from here. `silo`
//! names its own prefix in its module info, the gateway reads it out of the
//! module list it is building the route from, and the gate is built with it.
//! A module moved or renamed moves this with it. The limit is the forwarding
//! limit rather than the object store's own: how large an object the leader
//! will keep is not how large a body a follower can hold to pass on.

/// The prefix `silo` asks the gateway to mount it on, as shipped. A gate
/// built without being told — every in-process test of these layers, and any
/// tool that mounts the gateway's modules its own way — carries on this.
const SILO_PREFIX: &str = "silo";

/// What the gate knows about the S3 API: where it answers, and the largest
/// body a follower holds for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct S3Carry {
    /// The address space the S3 API answers on: the module's prefix with its
    /// leading slash, `/silo` as shipped. Empty carries nothing.
    mount: String,
    /// The largest body a follower holds to pass an S3 write on.
    limit: usize,
}

impl Default for S3Carry {
    /// The shipped mount point and the forwarding limit.
    fn default() -> Self {
        Self {
            mount: format!("/{SILO_PREFIX}"),
            limit: super::FORWARD_LIMIT,
        }
    }
}

impl S3Carry {
    /// A gate that carries writes sent to `prefix`, up to `limit` bytes.
    pub fn new(prefix: &str, limit: usize) -> Self {
        Self {
            mount: format!("/{prefix}"),
            limit,
        }
    }

    /// True when `path` is a request for the S3 API. The mount point itself
    /// counts: `PUT /silo` is aimed at that surface whatever answers it. A
    /// path that merely starts with the name — `/siloevil/k` — is not.
    pub fn carries(&self, path: &str) -> bool {
        !self.mount.is_empty()
            && (path == self.mount || path.starts_with(&format!("{}/", self.mount)))
    }

    /// The largest body this server carries an S3 write at.
    pub fn limit(&self) -> usize {
        self.limit
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_path_on_the_s3_mount_is_carried() {
        let mount = S3Carry::new("silo", 8);
        assert!(mount.carries("/silo/widgets/k.txt"));
        // The mount point itself is that surface, whatever answers it.
        assert!(mount.carries("/silo"));
        // A name that merely starts with the mount belongs to something else.
        assert!(!mount.carries("/siloevil/k"));
        assert!(!mount.carries("/identity/projects"));
        assert!(!mount.carries("/"));
        // A gate mounted somewhere else carries that surface and nothing more.
        let renamed = S3Carry::new("objects", 8);
        assert!(renamed.carries("/objects/widgets/k.txt"));
        assert!(!renamed.carries("/silo/widgets/k.txt"));
    }

    /// The limit a follower carries an S3 body at is the limit every other
    /// carried write is held to, and it is at least as large as the biggest
    /// part a default multipart upload sends — which is what an S3 client
    /// pointed at a follower uploads in.
    #[test]
    fn the_default_gate_holds_a_default_multipart_part() {
        let gate = S3Carry::default();
        assert!(gate.carries("/silo"));
        assert_eq!(gate.limit(), super::super::FORWARD_LIMIT);
        assert!(gate.limit() >= silo::multipart::MIN_PART_BYTES);
        assert!(gate.carries("/silo/widgets/k.txt"));
    }
}
