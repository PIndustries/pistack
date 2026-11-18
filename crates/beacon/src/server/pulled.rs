// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::net::IpAddr;

/// What an accepted NOTIFY hands the pull loop: the apex, and the address the
/// NOTIFY came from, when the caller knew one.
pub(crate) type Pulled = (String, Option<IpAddr>);
