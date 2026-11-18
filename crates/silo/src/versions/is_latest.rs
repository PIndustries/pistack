// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::Object;

use super::head::head;

/// True when `obj` is the newest generation of its key — the row ListObjectVersions
/// answers `IsLatest` for. Taken from the history rather than `status.latest`,
/// because a marker left on top of a key makes that key's newest generation
/// while the record under it still carries the flag.
pub(crate) fn is_latest(all: &[Object], obj: &Object) -> bool {
    head(all, &obj.spec.key).is_some_and(|cur| cur.status.version == obj.status.version)
}
