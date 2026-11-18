// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::time::Duration;

/// Extra wait mixed in so two followers do not increment the same term.
pub const ELECTION_TIMEOUT_SPAN: Duration = Duration::from_millis(1_200);
