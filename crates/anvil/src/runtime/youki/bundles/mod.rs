// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

#[path = "checked_removal_path.rs"]
mod checked_removal_path;
pub(in crate::runtime::youki) use checked_removal_path::*;
#[path = "cleanup_helper.rs"]
mod cleanup_helper;
pub(in crate::runtime::youki) use cleanup_helper::*;
#[path = "delete_bundle.rs"]
mod delete_bundle;
pub(in crate::runtime::youki) use delete_bundle::*;
#[path = "delete_path.rs"]
mod delete_path;
pub(in crate::runtime::youki) use delete_path::*;
#[path = "normalize_lexical.rs"]
mod normalize_lexical;
pub(in crate::runtime::youki) use normalize_lexical::*;
#[path = "path_arg.rs"]
mod path_arg;
pub(in crate::runtime::youki) use path_arg::*;
#[path = "path_exists.rs"]
mod path_exists;
pub(in crate::runtime::youki) use path_exists::*;
#[path = "permission_denied.rs"]
mod permission_denied;
pub(in crate::runtime::youki) use permission_denied::*;
#[path = "privatize_bundle.rs"]
mod privatize_bundle;
pub(in crate::runtime::youki) use privatize_bundle::*;
#[path = "privileged_remove.rs"]
mod privileged_remove;
pub(in crate::runtime::youki) use privileged_remove::*;
#[path = "read_state.rs"]
mod read_state;
pub(in crate::runtime::youki) use read_state::*;
#[path = "removal_text_ok.rs"]
mod removal_text_ok;
pub(in crate::runtime::youki) use removal_text_ok::*;
#[path = "remove_bundle.rs"]
mod remove_bundle;
pub(in crate::runtime::youki) use remove_bundle::*;
#[path = "remove_err.rs"]
mod remove_err;
pub(in crate::runtime::youki) use remove_err::*;
#[path = "remove_unprivileged.rs"]
mod remove_unprivileged;
pub(in crate::runtime::youki) use remove_unprivileged::*;
#[path = "write_state.rs"]
mod write_state;
pub(in crate::runtime::youki) use write_state::*;

#[cfg(test)]
#[path = "sudo_mode_removes_a_bundle_the_agent_cannot_unlink_tests.rs"]
mod sudo_mode_removes_a_bundle_the_agent_cannot_unlink_tests;

#[cfg(test)]
#[path = "youki_cleanup_helper_self_test_tests.rs"]
mod youki_cleanup_helper_self_test_tests;
