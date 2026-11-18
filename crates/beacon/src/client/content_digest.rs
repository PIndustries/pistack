// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::*;
use std::collections::BTreeMap;

use super::set_shape::SetShape;

/// Stable fingerprint of everything that ends up on the wire: zone spec plus
/// every set's owner, type, ttl and records, in a canonical order.
pub(crate) fn content_digest(zone: &Zone, sets: &[RecordSet]) -> String {
    use std::hash::{Hash, Hasher};
    let mut ordered: BTreeMap<(String, &str), SetShape> = BTreeMap::new();
    for s in sets {
        let mut recs = s.spec.records.clone();
        recs.sort();
        ordered.insert(
            (s.spec.name.clone(), s.spec.kind.as_str()),
            (s.spec.ttl, recs),
        );
    }
    let mut h = std::collections::hash_map::DefaultHasher::new();
    zone.spec.name.hash(&mut h);
    zone.spec.ttl.hash(&mut h);
    zone.spec.email.hash(&mut h);
    zone.nameservers().hash(&mut h);
    ordered.hash(&mut h);
    format!("{:016x}", h.finish())
}
