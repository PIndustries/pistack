// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::Object;

/// What one delete of a key does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Action {
    /// Records, file and blobs of the key all go. A bucket without versioning,
    /// or a versioned key whose current generation is already a marker.
    RemoveKey,
    /// Nothing is removed; a marker becomes the key's current generation.
    AddMarker,
}

/// Which of the two a delete of `key` is, once the key is known to be there.
pub(crate) fn action(bucket: &crate::types::Bucket, current: &Object) -> Action {
    match (bucket.spec.versioning, current.status.delete_marker) {
        // A live generation on a bucket that keeps generations: hide it.
        (true, false) => Action::AddMarker,
        // A bucket that keeps no generations: the key goes whole.
        (false, _) => Action::RemoveKey,
        // A marker is the current record and holds nothing, so there is
        // nothing left to hide: the delete takes the key and everything
        // under the marker.
        (true, true) => Action::RemoveKey,
    }
}
