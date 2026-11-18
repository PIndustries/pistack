// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::time::Duration;

/// How long an idle TCP client may hold a connection.
pub(crate) const TCP_TIMEOUT: Duration = Duration::from_secs(10);
