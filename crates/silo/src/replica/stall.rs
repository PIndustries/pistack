// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::time::Duration;

/// A peer that sends nothing for this long is given up on. There is no
/// limit on the whole transfer: a 5 GiB object takes as long as it takes.
pub(crate) const STALL: Duration = Duration::from_secs(30);
