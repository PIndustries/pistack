// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::machine_key::machine_key;
use pistack_types::Id;
/// The queue key of a machine's rebuild. A rebuild is not a start that may
/// be re-sent: its answer is keyed apart so a memoised start never stands
/// in for it, and a memoised rebuild never stands in for a later start.
pub(super) fn rebuild_key(id: &Id) -> String {
    format!("{}/rebuild", machine_key(id))
}
