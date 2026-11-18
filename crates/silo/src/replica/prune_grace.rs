// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::time::Duration;

/// A follower leaves bytes with no record alone until they are this old,
/// so a record that is still on its way through the log is not mistaken
/// for a deleted one.
pub const PRUNE_GRACE: Duration = Duration::from_secs(15 * 60);
