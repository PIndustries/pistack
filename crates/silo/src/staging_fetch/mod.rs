// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! One server's calls about multipart staging to another.
//!
//! Every call here goes the way every call between control-plane servers
//! goes: through `keep::peer::PeerNet`, so it reaches the other server's
//! TLS listener when it has one, and carries the peer key. The path is
//! written against the address `PeerNet` chose, never against the address
//! the member list named.

mod ask_peer;
mod fetch_part;
mod fetch_upload;
mod get;
mod list_staged;
mod stall;
pub(crate) use ask_peer::ask_peer;
pub(crate) use fetch_part::fetch_part;
pub(crate) use fetch_upload::fetch_upload;
pub(crate) use list_staged::list_staged;
