// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::time::Duration;

/// How long to wait for the TCP connect and the transfer itself.
pub(crate) const XFER_TIMEOUT: Duration = Duration::from_secs(10);
