// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::Router;

/// The vpc's name, as the router it owns was told when it was made.
pub(crate) fn owner_name(r: &Router) -> Option<String> {
    r.meta.annotations.get("vpc").cloned()
}
