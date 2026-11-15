// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::probe::exec::command_probe_reply::CommandProbeReply;
use crate::probe::run::run;
use crate::probe::test_support::exec_check::exec_check;
use std::time::Duration;

#[tokio::test]
async fn exec_validation_does_not_call_the_runtime() {
    let called = std::sync::atomic::AtomicBool::new(false);
    let empty = exec_check(vec![], Duration::from_secs(1));
    let outcome = run(&empty, |_, _| {
        called.store(true, std::sync::atomic::Ordering::Relaxed);
        async {
            CommandProbeReply::Finished {
                exit_code: 0,
                output: String::new(),
            }
        }
    })
    .await;
    assert!(!called.load(std::sync::atomic::Ordering::Relaxed));
    assert!(!outcome.success);
    assert!(!outcome.unsupported);
    assert_eq!(outcome.message, "exec command is empty");

    let nul = exec_check(vec![format!("a{}b", '\0')], Duration::from_secs(1));
    let outcome = run(&nul, |_, _| async {
        CommandProbeReply::Finished {
            exit_code: 0,
            output: String::new(),
        }
    })
    .await;
    assert_eq!(outcome.message, "exec command contains a NUL byte");
    assert!(!outcome.success);
}
