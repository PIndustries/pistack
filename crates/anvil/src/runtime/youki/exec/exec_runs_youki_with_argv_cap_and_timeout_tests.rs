// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::MachineRuntime;

use pistack_types::error::ErrorCode;

use crate::runtime::youki::consts::EXEC_OUTPUT_CAP;
use crate::runtime::youki::runtime::YoukiRuntime;
use crate::runtime::youki::test_support::arg_lines;
use crate::runtime::youki::test_support::count_arg;
use crate::runtime::youki::test_support::env_lock;
use crate::runtime::youki::test_support::has_seq;
use crate::runtime::youki::test_support::req;
use crate::runtime::youki::test_support::sandbox_doc;
use crate::runtime::youki::test_support::scopeguard_dir;
use crate::runtime::youki::test_support::write_exec_script;
use crate::runtime::youki::test_support::EnvGuard;
use pistack_types::kind;
use pistack_types::Id;
use std::time::Duration;

#[tokio::test]
async fn exec_runs_youki_with_argv_cap_and_timeout() {
    let _guard = env_lock().lock().await;
    let tmp = std::env::temp_dir().join(format!(
        "pistack-youki-exec-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&tmp).unwrap();
    let _dir = scopeguard_dir(&tmp);
    let log = tmp.join("youki.log");
    let bin = tmp.join("youki");
    let state = tmp.join("state");
    write_exec_script(&bin, &log);
    let _env = EnvGuard::set(&bin, &state);
    let rt = YoukiRuntime;

    let external = tmp.join("image");
    std::fs::create_dir_all(&external).unwrap();
    std::fs::write(external.join("marker"), "keep").unwrap();
    let image = external.to_str().unwrap();
    let mut request = req("web", "not-a-path", "10.1.0.8");
    request.user_data = Some(sandbox_doc(image));

    let absent = Id::new(kind::MACHINE);
    let err = rt
        .exec(
            &absent,
            "web",
            &["/bin/true".into()],
            &[],
            None,
            Duration::from_secs(2),
            64,
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound, "{err}");
    assert!(err.message.contains("not on this host"), "{err}");

    rt.start(&request).await.unwrap();
    let out = rt
        .exec(
            &request.id,
            "web",
            &["/bin/echo".into(), "hello".into()],
            &[
                ("MODE".into(), "prod".into()),
                ("EMPTY".into(), String::new()),
            ],
            Some("/work"),
            Duration::from_secs(5),
            EXEC_OUTPUT_CAP,
        )
        .await
        .unwrap();
    assert_eq!(out.exit_code, 7);
    assert_eq!(out.stdout, "out-line\n");
    assert_eq!(out.stderr, "err-line\n");
    let id0 = format!("{}-0", request.id);
    let lines = arg_lines(&std::fs::read_to_string(&log).unwrap());
    assert!(
        has_seq(
            &lines,
            &[
                "exec".into(),
                "--cwd".into(),
                "/work".into(),
                "--env".into(),
                "MODE=prod".into(),
                "--env".into(),
                "EMPTY=".into(),
                id0.clone(),
                "--".into(),
                "/bin/echo".into(),
                "hello".into(),
            ],
        ),
        "{lines:?}"
    );

    let side = rt
        .exec(
            &request.id,
            "side",
            &["/bin/true".into()],
            &[],
            None,
            Duration::from_secs(5),
            64,
        )
        .await
        .unwrap();
    assert_eq!(side.exit_code, 7);
    let id1 = format!("{}-1", request.id);
    let lines = arg_lines(&std::fs::read_to_string(&log).unwrap());
    assert!(
        has_seq(
            &lines,
            &["exec".into(), id1, "--".into(), "/bin/true".into(),],
        ),
        "{lines:?}"
    );

    let by_id = rt
        .exec(
            &request.id,
            &id0,
            &["/bin/true".into()],
            &[],
            None,
            Duration::from_secs(5),
            64,
        )
        .await
        .unwrap();
    assert_eq!(by_id.exit_code, 7);

    let flooded = rt
        .exec(
            &request.id,
            "web",
            &["__flood__".into()],
            &[],
            None,
            Duration::from_secs(5),
            16,
        )
        .await
        .unwrap();
    assert_eq!(flooded.exit_code, 0);
    assert_eq!(flooded.stdout, "x".repeat(16));
    assert_eq!(flooded.stderr, "y".repeat(16));

    let started = std::time::Instant::now();
    let err = rt
        .exec(
            &request.id,
            "web",
            &["__sleep__".into()],
            &[],
            None,
            Duration::from_millis(400),
            64,
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::Unavailable, "{err}");
    assert!(err.message.contains("timed out"), "{err}");
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "exec timeout took {:?}",
        started.elapsed()
    );

    let before = count_arg(&arg_lines(&std::fs::read_to_string(&log).unwrap()), "exec");
    let err = rt
        .exec(
            &request.id,
            "nope",
            &["/bin/true".into()],
            &[],
            None,
            Duration::from_secs(2),
            64,
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound, "{err}");
    assert!(err.message.contains("nope"), "{err}");
    let err = rt
        .exec(
            &request.id,
            "web",
            &[],
            &[],
            None,
            Duration::from_secs(2),
            64,
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::BadRequest, "{err}");
    assert!(err.message.contains("empty"), "{err}");
    let after = count_arg(&arg_lines(&std::fs::read_to_string(&log).unwrap()), "exec");
    assert_eq!(before, after, "a rejected exec must not call youki");

    rt.stop(&request.id).await.unwrap();
    let err = rt
        .exec(
            &request.id,
            "web",
            &["/bin/true".into()],
            &[],
            None,
            Duration::from_secs(2),
            64,
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::Unavailable, "{err}");
    assert!(err.message.contains("not running"), "{err}");

    let missing_bin = tmp.join("missing-youki");
    std::env::set_var("PISTACK_YOUKI_BIN", &missing_bin);
    let err = rt
        .exec(
            &request.id,
            "web",
            &["/bin/true".into()],
            &[],
            None,
            Duration::from_secs(2),
            64,
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::Unavailable, "{err}");
    assert!(err.message.contains("not available"), "{err}");

    let logged = std::fs::read_to_string(&log).unwrap();
    assert!(!logged.contains("do-not-log-user-data"), "{logged}");
    assert!(!logged.contains("do-not-log-ssh-key"), "{logged}");
}
