// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::probe::consts::{PROBE_EXEC_OUTPUT_CAP, PROBE_MESSAGE_CAP};
use crate::probe::exec::command_probe_reply::CommandProbeReply;
use crate::probe::exec::reply_from_exec::reply_from_exec;
use crate::probe::outcome_of_reply;
use crate::probe::run::run;
use crate::probe::test_support::exec_check::exec_check;
use pistack_types::ApiError;
use std::time::Duration;

#[tokio::test]
async fn exec_probe_maps_exit_timeout_and_drops_secrets() {
    let check = exec_check(vec!["/bin/check".into()], Duration::from_secs(2));
    let ok = run(&check, |argv, timeout| {
        assert_eq!(argv, &["/bin/check".to_string()]);
        assert_eq!(timeout, Duration::from_secs(2));
        async {
            CommandProbeReply::Finished {
                exit_code: 0,
                output: "password=hunter2".into(),
            }
        }
    })
    .await;
    assert!(ok.success);
    assert!(!ok.unsupported);
    assert!(ok.message.is_empty());

    let failed = run(&check, |_, _| async {
        CommandProbeReply::Finished {
            exit_code: 7,
            output: "password=hunter2 status=down".into(),
        }
    })
    .await;
    assert!(!failed.success);
    assert!(!failed.unsupported);
    assert!(failed.message.starts_with("exit 7"), "{}", failed.message);
    assert!(failed.message.contains("status=down"), "{}", failed.message);
    assert!(!failed.message.contains("hunter2"), "{}", failed.message);
    assert!(failed.message.contains("***"), "{}", failed.message);

    let timed_out = run(&check, |_, _| async { CommandProbeReply::TimedOut }).await;
    assert!(!timed_out.success);
    assert!(!timed_out.unsupported);
    assert_eq!(timed_out.message, "timed out");

    let from_youki = outcome_of_reply(reply_from_exec(Err(ApiError::unavailable(
        "youki runtime: exec timed out after 1s",
    ))));
    assert_eq!(from_youki.message, "timed out");
    assert!(!from_youki.success);

    let missing = outcome_of_reply(reply_from_exec(Err(ApiError::not_found(
        "youki runtime: machine mach-1 is not on this host",
    ))));
    assert!(missing.unsupported);
    assert!(!missing.success);

    let key = outcome_of_reply(CommandProbeReply::Finished {
        exit_code: 1,
        output: "-----BEGIN OPENSSH PRIVATE KEY-----\nabc\n".into(),
    });
    assert_eq!(key.message, "exit 1");
    assert!(!key.message.contains("abc"));
    assert!(!key.message.contains("PRIVATE"));

    let leaked = outcome_of_reply(reply_from_exec(Err(ApiError::unavailable(
        "youki runtime: exec failed: token=s3cret-value",
    ))));
    assert!(!leaked.unsupported);
    assert!(
        !leaked.message.contains("s3cret-value"),
        "{}",
        leaked.message
    );

    let long = "status=down ".repeat(40);
    let truncated = run(&check, |_, _| {
        let output = long.clone();
        async move {
            CommandProbeReply::Finished {
                exit_code: 2,
                output,
            }
        }
    })
    .await;
    let excerpt = truncated
        .message
        .strip_prefix("exit 2: ")
        .expect("failure keeps the exit code");
    assert_eq!(excerpt.chars().count(), PROBE_MESSAGE_CAP);
    assert!(excerpt.contains("status=down"));
    assert!(excerpt.chars().count() < PROBE_EXEC_OUTPUT_CAP);
}
