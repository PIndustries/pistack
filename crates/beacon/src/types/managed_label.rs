// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

/// Label beacon puts on record sets it manages itself; users should not
/// edit those by hand, the next pass would overwrite them.
pub const MANAGED_LABEL: &str = "pistack.io/managed-by";
