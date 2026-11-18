// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::discover::Announce;

use super::formation::Formation;

/// `heard` may include this node; ignore entries whose node_id == self_id.
/// An empty cluster_id means "still forming", not a cluster to join.
/// One distinct non-empty cluster_id -> Join the announce with the smallest node_id.
/// Two or more -> Ambiguous with the ids sorted.
/// None -> Wait if any remaining peer has a smaller node_id, otherwise Found.
pub fn classify(self_id: &str, heard: &[Announce]) -> Formation {
    let peers: Vec<&Announce> = heard.iter().filter(|ann| ann.node_id != self_id).collect();
    let mut clusters: Vec<String> = peers
        .iter()
        .filter(|ann| !ann.cluster_id.is_empty())
        .map(|ann| ann.cluster_id.clone())
        .collect();
    clusters.sort();
    clusters.dedup();
    match clusters.len() {
        0 => {
            if peers.iter().any(|ann| ann.node_id.as_str() < self_id) {
                Formation::Wait
            } else {
                Formation::Found
            }
        }
        1 => {
            let chosen = peers
                .iter()
                .copied()
                .filter(|ann| !ann.cluster_id.is_empty())
                .min_by(|left, right| left.node_id.cmp(&right.node_id))
                .expect("one cluster id has a peer");
            Formation::Join(chosen.clone())
        }
        _ => Formation::Ambiguous(clusters),
    }
}
