// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::probe::book::book_type::Book;
use std::sync::Mutex;

/// In-memory probe streaks for every sandbox on this host.
#[derive(Default)]
pub struct ProbeBook {
    pub(super) inner: Mutex<Book>,
}
