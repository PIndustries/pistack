// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The options on one export line, and the address a client mounts.
//!
//! Every line the shelf writes carries the same options:
//!
//! ```text
//! <dir> <cidr>(ro|rw,sync,no_subtree_check,all_squash,anonuid=<u>,anongid=<u>)
//! ```
//!
//! `all_squash` with `anonuid`/`anongid` at the share directory's owner is
//! what makes the modes mean what they say: every client user, root
//! included, arrives at the share as that one uid. `rw` is given only where
//! the share allows the CIDR in mode `write`; a `read` rule exports `ro`.
//! `sync` and `no_subtree_check` were already there and stay.

use crate::share_dir::squash_owner;
use crate::types::AccessMode;

/// One export line for `dir` toward `cidr`, with the squashed owner taken
/// from the process environment (see [`crate::share_dir::squash_owner`]).
pub fn export_line(dir: &str, cidr: &str, mode: AccessMode) -> String {
    export_line_as(dir, cidr, mode, squash_owner())
}

/// The same line with the squashed uid given.
fn export_line_as(dir: &str, cidr: &str, mode: AccessMode, owner: u32) -> String {
    let flag = match mode {
        AccessMode::Read => "ro",
        AccessMode::Write => "rw",
    };
    format!(
        "{dir} {cidr}({flag},sync,no_subtree_check,all_squash,anonuid={owner},anongid={owner})\n"
    )
}

/// The host of a public URL, without the brackets of an IPv6 address and
/// without a port: the address a client puts before `:<path>` to mount.
/// `None` when the URL names no host.
pub fn address_of(url: &str) -> Option<String> {
    let rest = url.split_once("://").map(|r| r.1).unwrap_or(url);
    let authority = rest
        .split(['/', '?', '#'])
        .next()
        .unwrap_or("")
        .rsplit('@')
        .next()
        .unwrap_or("");
    let host = match authority.split_once(':') {
        // `[::1]:8443` splits at the colon inside the brackets; the port is
        // after the bracket, so cut at `]` first.
        Some((_, _)) if authority.starts_with('[') => authority
            .split_once(']')
            .map(|(h, _)| h.trim_matches('['))
            .unwrap_or(authority),
        Some((host, _)) => host,
        None => authority,
    };
    let host = host.trim();
    (!host.is_empty()).then(|| host.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_lock() -> &'static std::sync::Mutex<()> {
        crate::share_dir::test_env_lock()
    }

    #[test]
    fn export_line_options_name_the_squashed_owner() {
        // The default uid is read from the environment, which another test
        // in this binary may be holding at its own value.
        let _guard = env_lock().lock().unwrap();
        assert_eq!(
            export_line_as("/srv/share-1", "10.0.0.0/24", AccessMode::Read, 65534),
            "/srv/share-1 10.0.0.0/24(ro,sync,no_subtree_check,\
             all_squash,anonuid=65534,anongid=65534)\n"
        );
        assert_eq!(
            export_line_as("/srv/share-1", "10.1.0.0/24", AccessMode::Write, 65534),
            "/srv/share-1 10.1.0.0/24(rw,sync,no_subtree_check,\
             all_squash,anonuid=65534,anongid=65534)\n"
        );
    }

    #[test]
    fn export_line_follows_the_injected_uid() {
        let _guard = env_lock().lock().unwrap();
        let prev = std::env::var_os("PISTACK_SHELF_SQUASH_UID");
        std::env::set_var("PISTACK_SHELF_SQUASH_UID", "4242");
        let line = export_line("/srv/s", "10.0.0.0/24", AccessMode::Write);
        assert!(line.contains("anonuid=4242"), "{line}");
        assert!(line.contains("anongid=4242"), "{line}");
        match prev {
            Some(v) => std::env::set_var("PISTACK_SHELF_SQUASH_UID", v),
            None => std::env::remove_var("PISTACK_SHELF_SQUASH_UID"),
        }
    }

    #[test]
    fn address_is_the_host_of_the_url_alone() {
        assert_eq!(
            address_of("https://10.97.0.1:8443"),
            Some("10.97.0.1".into())
        );
        assert_eq!(
            address_of("http://cloud.example"),
            Some("cloud.example".into())
        );
        assert_eq!(
            address_of("http://cloud.example/"),
            Some("cloud.example".into())
        );
        assert_eq!(
            address_of("https://[fd00::5]:8443/base"),
            Some("fd00::5".into())
        );
        assert_eq!(
            address_of("https://user@cloud.example:8443"),
            Some("cloud.example".into())
        );
        assert_eq!(
            address_of("cloud.example:8443"),
            Some("cloud.example".into())
        );
        assert_eq!(address_of("https:///path"), None);
        assert_eq!(address_of(""), None);
    }
}
