// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

/// A root disk rebuild the reconciler owes the machine, written by
/// `POST /machines/{id}/rebuild`. The value is the image the new disk is a
/// copy of. While it is set, `Updating` means rebuild, not migration.
pub const REBUILD_ANNOTATION: &str = "pistack.io/rebuild";
