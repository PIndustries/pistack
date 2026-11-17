// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::server::ZoneData;

use super::pull_ordered::pull_ordered;

/// Try each primary in order until one AXFR succeeds.
pub(crate) async fn pull_from_primaries(
    primaries: &[String],
    apex: &str,
) -> anyhow::Result<(u32, ZoneData, String)> {
    pull_ordered(primaries, primaries, apex).await
}
