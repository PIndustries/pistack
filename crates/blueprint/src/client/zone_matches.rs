// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

/// Whether `zone_ref` (name, id, or apex) names `zone`.
pub(crate) fn zone_matches(zone: &beacon::Zone, zone_ref: &str) -> bool {
    let zone_ref = zone_ref.trim();
    zone_ref == zone.meta.name
        || zone_ref == zone.meta.id.to_string()
        || beacon::normalize_name(zone_ref).is_ok_and(|apex| apex == zone.spec.name)
}
