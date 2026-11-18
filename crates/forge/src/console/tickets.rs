// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::LazyLock;

use super::ticket::Ticket;

pub(crate) static TICKETS: LazyLock<Mutex<HashMap<String, Ticket>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
