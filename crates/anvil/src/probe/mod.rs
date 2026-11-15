// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Liveness and readiness checks run on the host that holds the pod.
//!
//! The agent reads probe definitions from the sandbox document (or from
//! the runtime, after a restart). `http_get` and `tcp_socket` connect to
//! the pod's sandbox address. `exec` runs its argv in the named container
//! through the runtime that holds the pod. youki uses the same exec helper
//! as `pi pod exec`, so `PISTACK_YOUKI_SUDO` and the rootless fallback apply
//! unchanged. Exit code 0 is a pass. A non-zero exit or a timeout is a
//! failure. Any other runtime reports the probe unsupported. An unsupported
//! result does not count as a failed check and is not a pass.
//!
//! Results are consecutive streaks reported on the ordinary machine
//! heartbeat. Period, timeout, and the success and failure thresholds are
//! the same for every handler. Captured command output is capped and
//! scrubbed before it is stored or logged. Probe env is not passed and is
//! not logged.

mod book;
pub use book::probe_book::ProbeBook;
pub use book::probe_target::ProbeTarget;
mod consts;
pub use consts::PROBE_EXEC_OUTPUT_CAP;
mod default_failure;
mod default_path;
mod default_period;
mod default_success;
mod default_timeout;
#[cfg(test)]
mod execute;

mod exec;
pub(crate) use exec::outcome_of_reply::outcome_of_reply;
pub use exec::reply_from_exec::reply_from_exec;
mod net;
mod nonzero;
mod probes_in_user_data;
pub use probes_in_user_data::probes_in_user_data;
#[cfg(test)]
mod probes_in_user_data_tests;
mod run;
pub use run::run;

mod sandbox_probe_container;
mod sandbox_probes;
mod spec;
pub use spec::container_probes::ContainerProbes;

#[cfg(test)]
mod test_support;
