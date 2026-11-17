// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `REVISION_HISTORY_LIMIT`: applied stack specs kept for rollback.
/// Applied stack specs kept for rollback. Older revisions are dropped.
pub const REVISION_HISTORY_LIMIT: usize = 10;
