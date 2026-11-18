// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::Client;
use pistack_types::{Id, Kind};

/// Kinds that cannot be in a vpc at all.
pub(crate) fn no_vpc(_: &Client, _: &impl Kind) -> Option<Id> {
    None
}
