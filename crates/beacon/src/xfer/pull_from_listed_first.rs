// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::server::ZoneData;

use super::pull_ordered::pull_ordered;

/// Pull from `first` before `rest` — the notify path names the sender, and
/// that is the primary whose data is known to be new. Addresses in `first`
/// are not tried twice.
pub(crate) async fn pull_from_listed_first(
    first: &[String],
    rest: &[String],
    apex: &str,
) -> anyhow::Result<(u32, ZoneData, String)> {
    pull_ordered(first, rest, apex).await
}
