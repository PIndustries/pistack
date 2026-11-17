// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The address a client of this server mounts NFS shares at.
//!
//! A share's status carries `<address>:<path>` once it is ready. The address
//! is the API host of the server that exports the share. The server process
//! knows its own public URL and hands it to modules through
//! [`pistack_types::ModuleContext`]; the shelf takes the host of that URL
//! when the module starts. `bin/pistack` also writes the host into
//! `PISTACK_EXPORT_ADDRESS` when the variable is unset, so a shelf run
//! outside the server process — the helper's own checks, a test — answers
//! the same question.
//!
//! The last non-empty write wins; an empty write changes nothing. When
//! nothing names an address the export field is simply absent: the local
//! path in `status.message` is all a client then gets, as before.

use std::sync::{Mutex, OnceLock};

/// `PISTACK_EXPORT_ADDRESS`: the address the environment names, if any.
pub const ENV: &str = "PISTACK_EXPORT_ADDRESS";

static ADDRESS: OnceLock<Mutex<Option<String>>> = OnceLock::new();
/// Set by the first [`set`], so a start that named no address (a test URL
/// with no host) keeps the environment from standing in for a server that
/// has no address to give.
static STARTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn slot() -> &'static Mutex<Option<String>> {
    ADDRESS.get_or_init(|| Mutex::new(None))
}

/// Remember `address` for the export field and mark this process as having
/// started the module, so [`get`] answers from this value alone. An empty
/// write records no address: a server whose URL names no host has no
/// address, and does not borrow one from the environment afterwards.
pub fn set(address: &str) {
    let address = address.trim();
    let remembered = (!address.is_empty()).then(|| address.to_owned());
    STARTED.store(true, std::sync::atomic::Ordering::Relaxed);
    *slot().lock().unwrap_or_else(|p| p.into_inner()) = remembered;
}

/// The address a ready share reports. A start of this module that named an
/// address wins for as long as it holds; otherwise the environment answers.
/// `bin/pistack` sets the variable for the server, and a test that pins it
/// after a start without a host in its URL is exactly that server.
pub fn get() -> Option<String> {
    if started() {
        return remembered();
    }
    remembered().or_else(env_address)
}

fn remembered() -> Option<String> {
    slot().lock().unwrap_or_else(|p| p.into_inner()).clone()
}

fn started() -> bool {
    STARTED.load(std::sync::atomic::Ordering::Relaxed)
}

/// `PISTACK_EXPORT_ADDRESS`, when the process has it.
fn env_address() -> Option<String> {
    std::env::var(ENV)
        .ok()
        .map(|v| v.trim().to_owned())
        .filter(|v| !v.is_empty())
}

/// Forget what [`set`] recorded and what the environment named, so a test
/// starts from nothing. `bin/pistack` sets this variable for the server the
/// same way [`set`] records it; a test that wants no address clears both.
#[cfg(test)]
pub fn clear() {
    *slot().lock().unwrap_or_else(|p| p.into_inner()) = None;
    STARTED.store(false, std::sync::atomic::Ordering::Relaxed);
    std::env::remove_var(ENV);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_environment_answers_until_a_start_names_an_address() {
        let prev = std::env::var_os(ENV);
        std::env::remove_var(ENV);
        clear();
        assert_eq!(get(), None);
        std::env::set_var(ENV, "10.97.0.1");
        assert_eq!(get().as_deref(), Some("10.97.0.1"));
        set("10.97.0.2");
        assert_eq!(get().as_deref(), Some("10.97.0.2"));
        // A second start that names no host wins over the environment: the
        // server it describes has no address to give.
        set("  ");
        assert_eq!(get(), None, "a start without a host borrows none");
        clear();
        std::env::remove_var(ENV);
        assert_eq!(get(), None);
        match prev {
            Some(v) => std::env::set_var(ENV, v),
            None => std::env::remove_var(ENV),
        }
    }
}
