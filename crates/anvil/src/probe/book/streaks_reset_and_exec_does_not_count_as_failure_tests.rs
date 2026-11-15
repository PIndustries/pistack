// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::probe::book::outcome::Outcome;
use crate::probe::book::probe_book::ProbeBook;
use crate::probe::spec::container_probes::ContainerProbes;
use crate::probe::spec::exec_spec::ExecSpec;
use crate::probe::spec::probe_spec::ProbeSpec;
use crate::probe::test_support::{later::later, spec::spec, t0::t0, target::target};
use pistack_rpc::proto::{PROBE_LIVENESS, PROBE_READINESS};
use pistack_types::{kind, Id};

#[test]
fn streaks_reset_and_exec_does_not_count_as_failure() {
    let book = ProbeBook::new();
    let id = Id::new(kind::MACHINE);
    let start = t0();
    book.install(
        &id,
        vec![ContainerProbes {
            container: "web".into(),
            liveness: Some(ProbeSpec {
                http_get: None,
                tcp_socket: None,
                exec: Some(ExecSpec {
                    command: vec!["/bin/true".into()],
                }),
                initial_delay_seconds: 0,
                period_seconds: 1,
                timeout_seconds: 1,
                success_threshold: 1,
                failure_threshold: 3,
            }),
            readiness: Some(spec(1, 0)),
        }],
        start,
    );
    let due = book.due(&[target(&id)], start);
    assert_eq!(due.len(), 2);
    for check in &due {
        if check.kind == PROBE_LIVENESS {
            book.record(
                check,
                &Outcome::unsupported("exec probes are not supported"),
            );
        } else {
            book.record(check, &Outcome::fail("http status 500"));
        }
    }
    let reports = book.reports(&id);
    let live = reports.iter().find(|r| r.kind == PROBE_LIVENESS).unwrap();
    assert_eq!(live.failures, 0);
    assert_eq!(live.successes, 0);
    assert!(live.unsupported);
    assert_eq!(live.message, "exec probes are not supported");
    let ready = reports.iter().find(|r| r.kind == PROBE_READINESS).unwrap();
    assert_eq!(ready.failures, 1);
    assert_eq!(ready.successes, 0);

    let again = book.due(&[target(&id)], later(start, 1));
    let ready_check = again.iter().find(|c| c.kind == PROBE_READINESS).unwrap();
    book.record(ready_check, &Outcome::ok());
    let ready = book
        .reports(&id)
        .into_iter()
        .find(|r| r.kind == PROBE_READINESS)
        .unwrap();
    assert_eq!(ready.successes, 1);
    assert_eq!(ready.failures, 0);
    assert!(ready.success);
}
