// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::probe::book::outcome::Outcome;
use crate::probe::book::probe_book::ProbeBook;
use crate::probe::spec::container_probes::ContainerProbes;
use crate::probe::test_support::{later::later, spec::spec, t0::t0, target::target};
use pistack_types::{kind, Id};

#[test]
fn install_ignores_a_result_from_the_previous_generation() {
    let book = ProbeBook::new();
    let id = Id::new(kind::MACHINE);
    let start = t0();
    book.install(
        &id,
        vec![ContainerProbes {
            container: "web".into(),
            liveness: Some(spec(1, 0)),
            readiness: None,
        }],
        start,
    );
    let stale = book.due(&[target(&id)], start);
    book.install(
        &id,
        vec![ContainerProbes {
            container: "web".into(),
            liveness: Some(spec(1, 0)),
            readiness: None,
        }],
        later(start, 1),
    );
    book.record(&stale[0], &Outcome::fail("late"));
    let report = &book.reports(&id)[0];
    assert_eq!(report.failures, 0);
    assert!(report.message.is_empty());
}
