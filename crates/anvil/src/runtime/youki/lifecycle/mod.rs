// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

#[path = "detach_io.rs"]
mod detach_io;
pub(in crate::runtime::youki) use detach_io::*;
#[path = "abort_new_pod.rs"]
mod abort_new_pod;
pub(in crate::runtime::youki) use abort_new_pod::*;
#[path = "arm_user_ns_join.rs"]
mod arm_user_ns_join;
pub(in crate::runtime::youki) use arm_user_ns_join::*;
#[path = "clear_leftover.rs"]
mod clear_leftover;
pub(in crate::runtime::youki) use clear_leftover::*;
#[path = "command_program.rs"]
mod command_program;
pub(in crate::runtime::youki) use command_program::*;
#[path = "container_missing.rs"]
mod container_missing;
pub(in crate::runtime::youki) use container_missing::*;
#[path = "create_container.rs"]
mod create_container;
pub(in crate::runtime::youki) use create_container::*;
#[path = "detach_stderr.rs"]
mod detach_stderr;
pub(in crate::runtime::youki) use detach_stderr::*;
#[path = "enter_user_ns.rs"]
mod enter_user_ns;
#[path = "forget_failed_create.rs"]
mod forget_failed_create;
pub(in crate::runtime::youki) use forget_failed_create::*;
#[path = "open_append.rs"]
mod open_append;
pub(in crate::runtime::youki) use open_append::*;
#[path = "parse_state_pid.rs"]
mod parse_state_pid;
pub(in crate::runtime::youki) use parse_state_pid::*;
#[path = "pause_stdio.rs"]
mod pause_stdio;
pub(in crate::runtime::youki) use pause_stdio::*;
#[path = "release_failed_create.rs"]
mod release_failed_create;
pub(in crate::runtime::youki) use release_failed_create::*;
#[path = "run_detach.rs"]
mod run_detach;
pub(in crate::runtime::youki) use run_detach::*;
#[path = "run_youki.rs"]
mod run_youki;
pub(in crate::runtime::youki) use run_youki::*;
#[path = "start_pod.rs"]
mod start_pod;
pub(in crate::runtime::youki) use start_pod::*;
#[path = "stderr_tail.rs"]
mod stderr_tail;
pub(in crate::runtime::youki) use stderr_tail::*;
#[path = "stdio_target.rs"]
mod stdio_target;
pub(in crate::runtime::youki) use stdio_target::*;
#[path = "workload_stdio.rs"]
mod workload_stdio;
pub(in crate::runtime::youki) use workload_stdio::*;
#[path = "youki_command.rs"]
mod youki_command;
pub(in crate::runtime::youki) use youki_command::*;

#[cfg(test)]
#[path = "create_deletes_a_leftover_container_before_create_tests.rs"]
mod create_deletes_a_leftover_container_before_create_tests;

#[cfg(test)]
#[path = "create_returns_while_a_child_holds_stdout_tests.rs"]
mod create_returns_while_a_child_holds_stdout_tests;

#[cfg(test)]
#[path = "create_timeout_and_failure_delete_force_and_drop_the_bundle_tests.rs"]
mod create_timeout_and_failure_delete_force_and_drop_the_bundle_tests;

#[cfg(test)]
#[path = "privilege_errors_name_sudo_and_the_user_namespace_tests.rs"]
mod privilege_errors_name_sudo_and_the_user_namespace_tests;

#[cfg(test)]
#[path = "start_is_idempotent_stop_kills_and_missing_bin_is_unavailable_tests.rs"]
mod start_is_idempotent_stop_kills_and_missing_bin_is_unavailable_tests;

#[cfg(test)]
#[path = "sudo_wraps_every_youki_command_and_root_ignores_it_tests.rs"]
mod sudo_wraps_every_youki_command_and_root_ignores_it_tests;
