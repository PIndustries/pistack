// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::bundle_dir::bundle_dir;
use crate::runtime::youki::bundles::read_state;
use crate::runtime::youki::exec::resolve_container;
use crate::runtime::youki::exec::run_exec;
use crate::runtime::youki::exec::validate_exec;
use crate::runtime::youki::resolve_run_mode::resolve_run_mode;
use crate::runtime::youki::resolve_youki::resolve_youki;
use crate::runtime::youki::runtime::lock;
use crate::runtime::youki::runtime::YoukiRuntime;
use crate::runtime::PowerState;
use pistack_rpc::proto::ExecOutput;
use pistack_types::ApiError;
use pistack_types::ApiResult;
use pistack_types::Id;
use std::time::Duration;

impl YoukiRuntime {
    #[allow(clippy::too_many_arguments)]
    /// Run `argv` with `youki exec` inside `container` of the pod sandbox `id`.
    ///
    /// The invocation follows the same privilege path as `create` and `start`:
    /// `sudo -n <youki> exec ...` when sudo mode is on, and `youki` directly
    /// when the agent is root or in the rootless fallback. `env` and `cwd`
    /// are optional: an empty slice and `None` leave the container's own
    /// environment and working directory. `timeout` bounds the wait.
    /// `output_cap` is the most bytes kept of stdout and of stderr,
    /// separately. The returned exit code is the command's, including
    /// non-zero. A missing youki binary is unavailable. A container this
    /// sandbox does not have, or a machine that is not on this host, is not
    /// found. Stdin is closed. This does not allocate a terminal.
    pub async fn exec(
        &self,
        id: &Id,
        container: &str,
        argv: &[String],
        env: &[(String, String)],
        cwd: Option<&str>,
        timeout: Duration,
        output_cap: usize,
    ) -> ApiResult<ExecOutput> {
        if timeout.is_zero() {
            return Err(ApiError::bad_request(
                "youki runtime: exec timeout must be greater than zero",
            ));
        }
        validate_exec(argv, env, cwd)?;
        let bin = resolve_youki()?;
        let container_id = {
            let _guard = lock().lock().await;
            let bundle = bundle_dir(id)?;
            let Some(saved) = read_state(&bundle).await? else {
                return Err(ApiError::not_found(format!(
                    "youki runtime: machine {id} is not on this host"
                )));
            };
            if saved.power != PowerState::On {
                return Err(ApiError::unavailable(
                    "youki runtime: pod sandbox is not running",
                ));
            }
            resolve_container(&saved, container)?.to_owned()
        };
        let mode = resolve_run_mode()?;
        run_exec(
            &bin,
            &mode.privilege,
            &container_id,
            argv,
            env,
            cwd,
            timeout,
            output_cap,
        )
        .await
    }
}
