// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::Id;

use super::api_state::ApiState;

/// The apex of the zone a record set lives in. `""` when it cannot be found:
/// the write itself already reported anything worth reporting.
pub(crate) async fn apex_of(st: &ApiState, zone: &Id) -> String {
    st.client
        .zones()
        .get(zone)
        .map(|z| z.spec.name)
        .unwrap_or_default()
}
