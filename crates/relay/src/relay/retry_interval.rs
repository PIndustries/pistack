// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! How often the reconciler retries resources that are not `Ready`.

use std::time::Duration;

/// How often the reconciler retries resources that are not `Ready`.
pub(crate) const RETRY_INTERVAL: Duration = Duration::from_secs(30);
