// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::peer_view::PeerView;

/// The peer to follow: one that says it leads, at our term or newer. The
/// newest term wins. A peer that leads an older term is stale.
pub fn pick_leader(term: u64, peers: &[PeerView]) -> Option<&PeerView> {
    peers
        .iter()
        .filter(|peer| peer.leader && peer.term >= term && !peer.api.is_empty())
        .max_by_key(|peer| peer.term)
}
