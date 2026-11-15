// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::probe::book::probe_book::ProbeBook;
use crate::probe::spec::container_probes::ContainerProbes;
use crate::probe::test_support::{later::later, spec::spec, t0::t0, target::target};
use pistack_rpc::proto::PROBE_LIVENESS;
use pistack_types::{kind, Id};

#[test]
fn scheduling_waits_for_delay_then_honors_period() {
    let book = ProbeBook::new();
    let id = Id::new(kind::MACHINE);
    let start = t0();
    book.install(
        &id,
        vec![ContainerProbes {
            container: "web".into(),
            liveness: Some(spec(10, 5)),
            readiness: None,
        }],
        start,
    );
    assert!(book.due(&[target(&id)], start).is_empty());
    assert!(book.due(&[target(&id)], later(start, 4)).is_empty());
    let first = book.due(&[target(&id)], later(start, 5));
    assert_eq!(first.len(), 1);
    assert!(book.due(&[target(&id)], later(start, 5)).is_empty());
    assert!(book.due(&[target(&id)], later(start, 14)).is_empty());
    let second = book.due(&[target(&id)], later(start, 15));
    assert_eq!(second.len(), 1);
    assert_eq!(second[0].kind, PROBE_LIVENESS);
}
