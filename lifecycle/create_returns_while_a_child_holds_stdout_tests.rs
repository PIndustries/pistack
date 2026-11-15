// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

    #[tokio::test]
    pub(super) async fn create_returns_while_a_child_holds_stdout() {
        let _guard = env_lock().lock().await;
        let tmp = temp_dir("stdio");
        let _dir = scopeguard_dir(&tmp);
        let log = tmp.join("youki.log");
        let bin = tmp.join("youki");
        let sleeper = tmp.join("sleeper");
        let quoted = shell_quote(&log);
        let sleeper_q = shell_quote(&sleeper);
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
  sleep 60 &
  echo $! > {sleeper_q}
  exit 0
fi
exit 0
"#
            ),
        );
        let state = tmp.join("state");
        let _env = EnvGuard::set(&bin, &state);
        let rt = YoukiRuntime;
        let request = plain_rootfs(&tmp);
        let started = std::time::Instant::now();
        let running = rt.start(&request).await.unwrap();
        let elapsed = started.elapsed();
        if let Ok(text) = std::fs::read_to_string(&sleeper) {
            if let Ok(pid) = text.trim().parse::<i32>() {
                if pid > 0 {
                    // SAFETY: `pid` is the positive pid this test spawned. A
                    // negative or zero pid would signal a process group.
                    unsafe { libc::kill(pid, libc::SIGKILL) };
                }
            }
        }
        assert_eq!(running.power, PowerState::On);
        assert!(
            elapsed < Duration::from_secs(5),
            "create waited for inherited stdout: {elapsed:?}"
        );
        let bundle = state.join(request.id.to_string());
        assert!(
            bundle.join("log").is_file(),
            "container stdio is the bundle log"
        );
        assert!(bundle.join("state.json").is_file());
    }
