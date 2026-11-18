// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::record_set::RecordSet;
use super::zone::Zone;

/// Render the zone in master-file format (RFC 1035 §5), SOA and apex NS
/// first, then every set sorted by owner and type.
pub fn export_zone(zone: &Zone, sets: &[RecordSet]) -> String {
    let mut out = String::new();
    let apex = &zone.spec.name;
    out.push_str(&format!("$ORIGIN {apex}\n$TTL {}\n", zone.spec.ttl));
    let ns = zone.nameservers();
    out.push_str(&format!(
        "{apex} {} IN SOA {} {} {} 7200 3600 1209600 {}\n",
        zone.spec.ttl,
        ns[0],
        zone.soa_rname(),
        zone.status.serial,
        zone.spec.ttl
    ));
    for n in &ns {
        out.push_str(&format!("{apex} {} IN NS {n}\n", zone.spec.ttl));
    }
    let mut sorted: Vec<&RecordSet> = sets.iter().collect();
    sorted.sort_by(|a, b| {
        a.spec
            .name
            .cmp(&b.spec.name)
            .then(a.spec.kind.as_str().cmp(b.spec.kind.as_str()))
    });
    for rs in sorted {
        let ttl = rs.spec.ttl.unwrap_or(zone.spec.ttl);
        for r in &rs.spec.records {
            out.push_str(&format!("{} {ttl} IN {} {r}\n", rs.spec.name, rs.spec.kind));
        }
    }
    out
}
