// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::metric_kind::MetricKind;
use super::reserved_prefix::RESERVED_PREFIX;
use super::table::CATALOG;

#[test]
fn names_are_reserved_legal_and_listed_once() {
    let mut seen = std::collections::BTreeSet::new();
    for m in CATALOG {
        assert!(m.name.starts_with(RESERVED_PREFIX), "{}", m.name);
        assert_eq!(crate::series::sanitize_name(m.name), m.name);
        assert!(seen.insert(m.name), "{} is listed twice", m.name);
        // Counters end in `_total`, and nothing else does.
        assert_eq!(
            m.kind == MetricKind::Counter,
            m.name.ends_with("_total"),
            "{}",
            m.name
        );
        assert!(!m.help.is_empty() && !m.help.contains('\n'));
    }
}
