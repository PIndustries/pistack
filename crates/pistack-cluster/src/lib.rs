// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! One control plane, several machines.
//!
//! A process with a store is already a one-voter cluster. The next machine
//! discovers it, or is pointed at it with `--join`, and replicates as a
//! learner. The third machine makes all three voters. Further machines
//! replicate as learners until an operator promotes them.

mod discover;
mod group;
mod replicate;

pub use discover::{
    check_advertise, collect_announcements, scan_targets, sweep, Announce, DiscoverError,
    DISCOVER_PORT,
};
pub use group::{admit, campaign, catch_up, propose, Member, Node, Role};
pub use replicate::{
    accepts_writes, classify, consider_vote, entries_after, join_membership, seed_membership,
    should_discover, Ballot, Formation, JoinRequest, StoredEntry, StoredOp, VoteRequest,
    VoteResponse,
};

/// What a starting server does with the peers it heard.
#[derive(Debug, PartialEq, Eq)]
pub enum BootChoice {
    /// This machine already has a store. Stay the voter you are and announce.
    Keep,
    /// `--init`, or a quiet prefix. Become the first voter.
    Init,
    /// `--join`, or exactly one cluster heard on the prefix.
    Join(Announce),
    /// Two clusters answered. Wait for `--join`.
    Ambiguous(Vec<String>),
}

/// Decide boot. An existing store always wins. `--init` and `--join` win
/// over discovery. An empty `join` is treated as absent.
pub fn choose(
    existing_store: bool,
    init: bool,
    join: Option<&str>,
    heard: &[Announce],
) -> BootChoice {
    if existing_store {
        return BootChoice::Keep;
    }
    if init {
        return BootChoice::Init;
    }
    if let Some(url) = join.map(str::trim).filter(|s| !s.is_empty()) {
        return BootChoice::Join(Announce {
            cluster_id: String::new(),
            node_id: String::new(),
            api: url.to_owned(),
            raft: String::new(),
        });
    }
    let mut clusters: Vec<String> = heard
        .iter()
        .map(|a| a.cluster_id.clone())
        .collect();
    clusters.sort();
    clusters.dedup();
    match clusters.as_slice() {
        [] => BootChoice::Init,
        [_] => {
            let mut peers: Vec<&Announce> = heard.iter().collect();
            peers.sort_by(|a, b| a.node_id.cmp(&b.node_id));
            BootChoice::Join(peers[0].clone())
        }
        _ => BootChoice::Ambiguous(clusters),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ann(cluster: &str, node: &str) -> Announce {
        Announce {
            cluster_id: cluster.into(),
            node_id: node.into(),
            api: format!("http://{node}"),
            raft: format!("{node}:18446"),
        }
    }

    #[test]
    fn an_existing_store_stays_put() {
        let heard = vec![ann("other", "b")];
        assert_eq!(
            choose(true, false, Some("http://manual"), &heard),
            BootChoice::Keep
        );
    }

    #[test]
    fn flags_beat_discovery_and_two_clusters_wait() {
        let heard = vec![ann("c1", "b"), ann("c2", "c"), ann("c1", "a")];
        assert_eq!(choose(false, true, None, &heard), BootChoice::Init);
        match choose(false, false, Some(" http://manual "), &heard) {
            BootChoice::Join(a) => assert_eq!(a.api, "http://manual"),
            other => panic!("{other:?}"),
        }
        assert_eq!(
            choose(false, false, None, &heard),
            BootChoice::Ambiguous(vec!["c1".into(), "c2".into()])
        );
        match choose(false, false, None, &[ann("c1", "b"), ann("c1", "a")]) {
            BootChoice::Join(a) => assert_eq!(a.node_id, "a"),
            other => panic!("{other:?}"),
        }
        assert_eq!(choose(false, false, None, &[]), BootChoice::Init);
    }
}
