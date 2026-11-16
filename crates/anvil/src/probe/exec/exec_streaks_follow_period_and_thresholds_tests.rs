// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::probe::book::probe_book::ProbeBook;
use crate::probe::exec::command_probe_reply::CommandProbeReply;
use crate::probe::run::run;
use crate::probe::spec::container_probes::ContainerProbes;
use crate::probe::spec::exec_spec::ExecSpec;
use crate::probe::spec::probe_spec::ProbeSpec;
use crate::probe::test_support::{later::later, t0::t0, target::target};
use chrono::{DateTime, Utc};
use pistack_rpc::proto::{PROBE_LIVENESS, PROBE_READINESS};
use pistack_types::{kind, Id};
use std::time::Duration;

#[tokio::test]
async fn exec_streaks_follow_period_and_thresholds() {
    let book = ProbeBook::new();
    let id = Id::new(kind::MACHINE);
    let start = t0();
    let spec = ProbeSpec {
        http_get: None,
        tcp_socket: None,
        exec: Some(ExecSpec {
            command: vec!["/bin/check".into()],
        }),
        initial_delay_seconds: 5,
        period_seconds: 10,
        timeout_seconds: 2,
        success_threshold: 2,
        failure_threshold: 3,
    };
    book.install(
        &id,
        vec![ContainerProbes {
            container: "web".into(),
            liveness: Some(spec.clone()),
            readiness: Some(spec.clone()),
        }],
        start,
    );
    assert!(book.due(&[target(&id)], start).is_empty());

    async fn settle(book: &ProbeBook, id: &Id, at: DateTime<Utc>, reply: CommandProbeReply) {
        for check in book.due(&[target(id)], at) {
            let outcome = run(&check, |argv, timeout| {
                assert_eq!(argv, &["/bin/check".to_string()]);
                assert_eq!(timeout, Duration::from_secs(2));
                let reply = reply.clone();
                async move { reply }
            })
            .await;
            book.record(&check, &outcome);
        }
    }

    settle(
        &book,
        &id,
        later(start, 5),
        CommandProbeReply::Finished {
            exit_code: 1,
            output: String::new(),
        },
    )
    .await;
    assert!(book.due(&[target(&id)], later(start, 5)).is_empty());
    assert!(book.due(&[target(&id)], later(start, 14)).is_empty());

    settle(&book, &id, later(start, 15), CommandProbeReply::TimedOut).await;
    let live = book
        .reports(&id)
        .into_iter()
        .find(|r| r.kind == PROBE_LIVENESS)
        .unwrap();
    assert_eq!(live.failures, 2);
    assert!(live.failures < spec.failure_threshold);
    assert!(!live.success);
    assert!(!live.unsupported);
    assert_eq!(live.message, "timed out");

    settle(
        &book,
        &id,
        later(start, 25),
        CommandProbeReply::Finished {
            exit_code: 9,
            output: String::new(),
        },
    )
    .await;
    let live = book
        .reports(&id)
        .into_iter()
        .find(|r| r.kind == PROBE_LIVENESS)
        .unwrap();
    assert_eq!(live.failures, spec.failure_threshold);
    assert_eq!(live.message, "exit 9");

    settle(
        &book,
        &id,
        later(start, 35),
        CommandProbeReply::Finished {
            exit_code: 0,
            output: String::new(),
        },
    )
    .await;
    let ready = book
        .reports(&id)
        .into_iter()
        .find(|r| r.kind == PROBE_READINESS)
        .unwrap();
    assert_eq!(ready.successes, 1);
    assert_eq!(ready.failures, 0);
    assert!(ready.successes < spec.success_threshold);
    assert!(ready.success);

    settle(
        &book,
        &id,
        later(start, 45),
        CommandProbeReply::Finished {
            exit_code: 0,
            output: String::new(),
        },
    )
    .await;
    let ready = book
        .reports(&id)
        .into_iter()
        .find(|r| r.kind == PROBE_READINESS)
        .unwrap();
    assert_eq!(ready.successes, spec.success_threshold);
    assert_eq!(ready.failures, 0);

    settle(&book, &id, later(start, 55), CommandProbeReply::Unsupported).await;
    let live = book
        .reports(&id)
        .into_iter()
        .find(|r| r.kind == PROBE_LIVENESS)
        .unwrap();
    assert!(live.unsupported);
    assert_eq!(live.failures, 0);
    assert_eq!(live.successes, 0);
    assert!(!live.success);
}
