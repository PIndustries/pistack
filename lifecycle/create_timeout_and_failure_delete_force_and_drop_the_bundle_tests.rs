// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

    #[tokio::test]
    pub(super) async fn create_timeout_and_failure_delete_force_and_drop_the_bundle() {
        let _guard = env_lock().lock().await;
        let tmp = temp_dir("cleanup");
        let _dir = scopeguard_dir(&tmp);
        let log = tmp.join("youki.log");
        let bin = tmp.join("youki");
        let quoted = shell_quote(&log);
        let shift = fake_shift_log();
        write_exe(
            &bin,
            &format!(
                r#"#!/bin/sh
printf '%s\n' "$@" >> {quoted}
printf '\n' >> {quoted}
{shift}
if [ "$cmd" = state ]; then
  echo "container $2 does not exist" >&2
  exit 1
fi
if [ "$cmd" = create ]; then
  sleep 30
  exit 0
fi
exit 0
"#
            ),
        );
        let state = tmp.join("state");
        let _env = EnvGuard::set(&bin, &state);
        std::env::set_var(TIMEOUT_ENV, "400");
        let rt = YoukiRuntime;
        let request = plain_rootfs(&tmp);
        let started = std::time::Instant::now();
        let err = rt.start(&request).await.unwrap_err();
        assert_eq!(err.code, ErrorCode::Unavailable, "{err}");
        assert!(err.message.contains("timed out"), "{err}");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "timeout took {:?}",
            started.elapsed()
        );
        let lines = arg_lines(&std::fs::read_to_string(&log).unwrap());
        assert!(
            lines
                .windows(2)
                .any(|w| w[0] == "delete" && w[1] == "--force"),
            "{lines:?}"
        );
        assert!(!state.join(request.id.to_string()).exists());

        write_exe(
            &bin,
            &format!(
                r#"#!/bin/sh
printf '%s\n' "$@" >> {quoted}
printf '\n' >> {quoted}
{shift}
if [ "$cmd" = state ]; then
  echo "container $2 does not exist" >&2
  exit 1
fi
if [ "$cmd" = create ]; then
  echo 'youki exploded' >&2
  exit 1
fi
exit 0
"#
            ),
        );
        let before = std::fs::read_to_string(&log).unwrap().len();
        let again = plain_rootfs(&tmp);
        let err = rt.start(&again).await.unwrap_err();
        assert!(err.message.contains("youki exploded"), "{err}");
        let lines = arg_lines(&std::fs::read_to_string(&log).unwrap()[before..]);
        assert!(
            lines
                .windows(2)
                .any(|w| w[0] == "delete" && w[1] == "--force"),
            "{lines:?}"
        );
        assert!(!state.join(again.id.to_string()).exists());
    }
