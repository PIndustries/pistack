// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! A member that cannot follow the leader's log replaces its own store.
//!
//! The leader refuses a pull in two ways no later pull can get past:
//! "diverged" (this store holds entries the leader never had, because this
//! server led and wrote them without a majority) and "compacted" (the
//! leader no longer keeps the entries this store needs). Either way the
//! only store this server can follow from is a copy of the leader's.
//!
//! The follow loop does that itself, in this order: download a snapshot to
//! `state.resync`, check it, give it this server's id, term and vote, move
//! `state.redb` aside under a dated name, move the snapshot into its place,
//! and ask the process to start again. The old store is kept because it
//! holds the writes nobody else has.
//!
//! The process starts again and the store is not swapped under it. Every
//! module holds the old `Store` handle and whatever it read from it, some
//! of it from when this server led, and the new store may be behind that.
//! A fresh process reads only the new one. Between the swap and the
//! restart the handle is sealed: it gives no vote and takes no term, since
//! either would be written to the file that was just set aside.
//!
//! The server keeps its member record. A voter comes back a voter: it
//! carries its own term and vote over, and the snapshot is the log of a
//! leader of that term or a newer one, so it holds every write this server
//! ever confirmed.

use super::*;

/// The checked snapshot, before it becomes `state.redb`.
pub(super) const RESYNC_FILE: &str = "state.resync";
/// A replaced store is `state.redb.replaced-<time>`.
const ASIDE_PREFIX: &str = "state.redb.replaced-";
const ASIDE_STAMP: &str = "%Y%m%dT%H%M%SZ";
/// Replaced stores kept. Older ones are deleted.
const ASIDE_KEEP: usize = 2;
/// A store is not replaced again this soon after the last time.
pub(super) const RESYNC_GAP: Duration = Duration::from_secs(600);
/// Wait after a failed attempt. Doubles up to [`RESYNC_GAP`].
const RESYNC_RETRY: Duration = Duration::from_secs(30);
/// Turns this off: the server logs what to do and waits for the operator.
pub const AUTO_RESYNC_ENV: &str = "PISTACK_AUTO_RESYNC";

/// Why the leader refused the pull.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Why {
    Diverged,
    Compacted,
}

/// Read the reason out of a failed pull.
pub(super) fn why(error: &str) -> Option<Why> {
    if error.contains("compacted") {
        Some(Why::Compacted)
    } else if error.contains("diverged") {
        Some(Why::Diverged)
    } else {
        None
    }
}

/// `PISTACK_AUTO_RESYNC=0` is off. Anything else, or nothing, is on.
pub fn auto_resync_value(value: Option<&str>) -> bool {
    !matches!(value.map(str::trim), Some("0"))
}

#[derive(Default)]
pub struct ResyncState {
    /// Set by a refused pull, cleared by one that worked.
    pub(super) wanted: Option<Why>,
    /// No attempt before this.
    not_before: Option<Instant>,
    failures: u32,
    /// The wait for [`RESYNC_GAP`] was logged.
    held: bool,
}

/// What was lost with the old store.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Lost {
    Nothing,
    /// These entries were in the old log and are not in the new one.
    Entries {
        first: u64,
        last: u64,
    },
    /// The logs differ somewhere at or before `upto`; the rows that would
    /// say where were trimmed.
    Unknown {
        upto: u64,
        last: u64,
    },
}

#[derive(Debug)]
pub(super) struct Replaced {
    pub aside: PathBuf,
    pub old_commit: u64,
    pub new_commit: u64,
    pub lost: Lost,
    pub role: Role,
}

fn data_dir(handle: &ClusterHandle) -> &Path {
    handle.db_path.parent().unwrap_or(Path::new("."))
}

/// True when the follow loop should replace the store now.
pub(super) fn due(handle: &ClusterHandle) -> bool {
    if !handle.auto_resync.load(Ordering::Relaxed)
        || handle.leader.load(Ordering::Acquire)
        || handle.removed.load(Ordering::Relaxed)
        || handle.sealed.load(Ordering::Acquire)
    {
        return false;
    }
    let mut state = guard(&handle.resync);
    if state.wanted.is_none() {
        return false;
    }
    if state.not_before.is_some_and(|at| Instant::now() < at) {
        return false;
    }
    let gap = *guard(&handle.resync_gap);
    let last = last_replaced(data_dir(handle));
    if let Some(wait) = held_for(last, SystemTime::now(), gap) {
        if !state.held {
            state.held = true;
            tracing::warn!(
                node = %handle.node_id,
                wait_secs = wait.as_secs(),
                "this server replaced its store a short while ago and the leader refuses it again; it will not try again before the wait is over. To do it by hand: stop this node, move state.redb aside (keep node.id), and start it with --join <leader api>"
            );
        }
        state.not_before = Some(Instant::now() + wait.min(Duration::from_secs(30)));
        return false;
    }
    state.held = false;
    true
}

/// How long the last replacement still holds the next one off.
fn held_for(last: Option<SystemTime>, now: SystemTime, gap: Duration) -> Option<Duration> {
    let since = now.duration_since(last?).unwrap_or(Duration::ZERO);
    gap.checked_sub(since).filter(|left| !left.is_zero())
}

/// Replaced stores in `dir`, newest first, with the time in each name.
fn asides(dir: &Path) -> Vec<(SystemTime, PathBuf)> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut found: Vec<(SystemTime, PathBuf)> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let stamp = name.strip_prefix(ASIDE_PREFIX)?;
            // A second file made in the same second ends `-2`.
            let stamp = stamp.get(..16)?;
            let at = chrono::NaiveDateTime::parse_from_str(stamp, ASIDE_STAMP).ok()?;
            Some((SystemTime::from(at.and_utc()), entry.path()))
        })
        .collect();
    found.sort_by(|a, b| b.cmp(a));
    found
}

fn last_replaced(dir: &Path) -> Option<SystemTime> {
    asides(dir).first().map(|(at, _)| *at)
}

fn aside_name(dir: &Path, now: chrono::DateTime<chrono::Utc>) -> PathBuf {
    let base = format!("{ASIDE_PREFIX}{}", now.format(ASIDE_STAMP));
    let mut path = dir.join(&base);
    let mut n = 2;
    while path.exists() {
        path = dir.join(format!("{base}-{n}"));
        n += 1;
    }
    path
}

/// Keep the newest [`ASIDE_KEEP`] replaced stores.
fn prune_asides(dir: &Path) {
    for (_, path) in asides(dir).into_iter().skip(ASIDE_KEEP) {
        match std::fs::remove_file(&path) {
            Ok(()) => tracing::info!(file = %path.display(), "deleted an older replaced store"),
            Err(err) => {
                tracing::warn!(file = %path.display(), error = %err, "could not delete an older replaced store")
            }
        }
    }
}

/// Replace the store with a snapshot from `leader`. True when it was done
/// and the process must start again.
pub(super) async fn run(handle: &std::sync::Arc<ClusterHandle>, leader: &str) -> bool {
    let Some(why) = guard(&handle.resync).wanted else {
        return false;
    };
    tracing::warn!(
        node = %handle.node_id,
        leader = %leader,
        reason = ?why,
        "replacing this server's store with a snapshot from the leader"
    );
    match attempt(handle, leader).await {
        Ok(done) => {
            let aside = done.aside.display().to_string();
            match done.lost {
                Lost::Nothing => tracing::warn!(
                    node = %handle.node_id,
                    old_index = done.old_commit,
                    new_index = done.new_commit,
                    role = ?done.role,
                    kept = %aside,
                    "store replaced: the old one was behind and nothing in it was lost. Starting again on the new store"
                ),
                Lost::Entries { first, last } => tracing::warn!(
                    node = %handle.node_id,
                    lost_first = first,
                    lost_last = last,
                    new_index = done.new_commit,
                    role = ?done.role,
                    kept = %aside,
                    "store replaced: log entries {first} to {last} of the old store were never confirmed by a majority and are not in the new one. The old store is kept. Starting again on the new store"
                ),
                Lost::Unknown { upto, last } => tracing::warn!(
                    node = %handle.node_id,
                    differs_at_or_before = upto,
                    lost_last = last,
                    new_index = done.new_commit,
                    role = ?done.role,
                    kept = %aside,
                    "store replaced: the old log differs from the leader's somewhere at or before entry {upto} and ran to entry {last}; those entries are not in the new store. The old store is kept. Starting again on the new store"
                ),
            }
            true
        }
        Err(err) => {
            let _ = std::fs::remove_file(data_dir(handle).join(RESYNC_FILE));
            if says_removed(&err) {
                note_removed(handle);
                return false;
            }
            let mut state = guard(&handle.resync);
            state.failures = state.failures.saturating_add(1);
            let wait = RESYNC_RETRY
                .saturating_mul(1 << state.failures.min(6).saturating_sub(1))
                .min(RESYNC_GAP);
            state.not_before = Some(Instant::now() + wait);
            tracing::warn!(
                node = %handle.node_id,
                leader = %leader,
                error = %format!("{err:#}"),
                retry_secs = wait.as_secs(),
                "could not replace this server's store; the old one is untouched"
            );
            false
        }
    }
}

async fn attempt(handle: &std::sync::Arc<ClusterHandle>, leader: &str) -> anyhow::Result<Replaced> {
    let incoming = data_dir(handle).join(RESYNC_FILE);
    let _ = std::fs::remove_file(&incoming);
    // The leader answered a moment ago, so there is no long wait for it.
    let limits = SnapshotFetch {
        startup: Duration::from_secs(5),
        restarts: 1,
        ..SnapshotFetch::default()
    };
    fetch_snapshot_as(leader, &incoming, &limits, &handle.node_id).await?;
    // The download may have taken long. Ask again who leads.
    let url = format!("{}/cluster", leader.trim_end_matches('/'));
    let status: ClusterStatus = rpc_get_json(&url, &handle.node_id).await?;
    if !status.leader {
        anyhow::bail!("{leader} stopped leading while the snapshot came down");
    }
    let handle = handle.clone();
    tokio::task::spawn_blocking(move || install(&handle, &incoming, &status)).await?
}

/// Check the snapshot, make it this server's, and swap the files.
fn install(
    handle: &ClusterHandle,
    incoming: &Path,
    leader: &ClusterStatus,
) -> anyhow::Result<Replaced> {
    let live = &handle.db_path;
    let (events, _) = tokio::sync::broadcast::channel(1);
    let new = Store::open(incoming, events)?;
    let cluster: Option<String> = new.get_meta("cluster_id")?;
    if cluster.as_deref() != Some(handle.cluster_id.as_str()) {
        anyhow::bail!("the snapshot is of another cluster");
    }
    let owner: Option<String> = new.get_meta("node_id")?;
    if owner.as_deref() != Some(leader.node_id.as_str()) {
        anyhow::bail!("the snapshot was not written by the leader");
    }
    let removed: Vec<String> = new.get_meta(REMOVED_KEY)?.unwrap_or_default();
    if removed.contains(&handle.node_id) {
        anyhow::bail!("the leader's store says this server was {REMOVED_MARK}");
    }
    let members: Vec<Member> = new.get_meta("members")?.unwrap_or_default();
    let Some(me) = members.iter().find(|member| member.id == handle.node_id) else {
        anyhow::bail!("the leader's member list does not name this server");
    };
    let role = me.role;

    // No vote and no term change from here to the end.
    let _ballot = guard(&handle.ballot);
    if handle.leader.load(Ordering::Acquire) {
        anyhow::bail!("this server leads; its store stays");
    }
    let ours = load_ballot(&handle.store)?;
    let their_term = new.get_meta::<u64>("cluster_term")?.unwrap_or(0);
    if their_term < ours.current_term {
        // A leader of an older term may lack writes this server confirmed.
        anyhow::bail!(
            "the snapshot is from term {their_term}, this server is at term {}",
            ours.current_term
        );
    }
    let voted = if their_term == ours.current_term {
        ours.voted_for.clone()
    } else {
        None
    };
    let old_commit = ours.last_index;
    let new_commit = new.commit_index()?;
    let lost = lost_entries(&handle.store, &new)?;
    new.put_meta("node_id", &handle.node_id)?;
    new.put_meta("cluster_voted_for", &voted)?;
    // Close it before it is renamed and opened again.
    drop(new);

    let dir = data_dir(handle);
    let aside = aside_name(dir, chrono::Utc::now());
    handle.sealed.store(true, Ordering::Release);
    if let Err(err) = std::fs::rename(live, &aside) {
        handle.sealed.store(false, Ordering::Release);
        return Err(err).with_context(|| format!("setting {} aside", live.display()));
    }
    if let Err(err) = std::fs::rename(incoming, live) {
        // Put the old store back. If that fails too, the next start finds
        // the snapshot and finishes the move.
        if std::fs::rename(&aside, live).is_ok() {
            handle.sealed.store(false, Ordering::Release);
        }
        return Err(err).with_context(|| format!("moving the snapshot to {}", live.display()));
    }
    if let Ok(dir) = std::fs::File::open(dir) {
        let _ = dir.sync_all();
    }
    prune_asides(dir);
    Ok(Replaced {
        aside,
        old_commit,
        new_commit,
        lost,
        role,
    })
}

/// Which entries of `old` are not in `new`. Walks back from the shorter tip
/// to the last index both logs hold with the same term.
fn lost_entries(old: &Store, new: &Store) -> anyhow::Result<Lost> {
    let last = old.commit_index()?;
    let mut at = last.min(new.commit_index()?);
    while at > 0 {
        match (old.log_term_at(at)?, new.log_term_at(at)?) {
            (Some(ours), Some(theirs)) if ours == theirs => break,
            (Some(_), Some(_)) => at -= 1,
            // A trimmed row. Entries below the new log's start are part of
            // the state the snapshot was made from.
            _ if at < new.log_start()? => break,
            _ => return Ok(Lost::Unknown { upto: at, last }),
        }
    }
    if at >= last {
        return Ok(Lost::Nothing);
    }
    Ok(Lost::Entries {
        first: at + 1,
        last,
    })
}

/// A start that finds a snapshot and no store finishes the move that was
/// cut short. A snapshot beside a store is a leftover and goes.
pub(super) fn finish_interrupted(data_dir: &Path) {
    let incoming = data_dir.join(RESYNC_FILE);
    if !incoming.exists() {
        return;
    }
    let live = data_dir.join("state.redb");
    if live.exists() {
        let _ = std::fs::remove_file(&incoming);
        return;
    }
    match std::fs::rename(&incoming, &live) {
        Ok(()) => tracing::warn!(
            "a store replacement was cut short; continuing on the snapshot it had downloaded"
        ),
        Err(err) => tracing::warn!(error = %err, "could not move state.resync into place"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_zero_turns_it_off() {
        assert!(auto_resync_value(None));
        assert!(auto_resync_value(Some("1")));
        assert!(auto_resync_value(Some("")));
        assert!(!auto_resync_value(Some("0")));
        assert!(!auto_resync_value(Some(" 0 ")));
    }

    #[test]
    fn a_refused_pull_names_its_reason() {
        assert_eq!(
            why("leader refused the pull: diverged at index 9: follower term 6, leader term 7"),
            Some(Why::Diverged)
        );
        assert_eq!(
            why("leader refused the pull: compacted: this server needs entries after 3"),
            Some(Why::Compacted)
        );
        assert_eq!(why("connection refused"), None);
    }

    #[test]
    fn the_last_replacement_holds_the_next_one_off() {
        let gap = Duration::from_secs(600);
        let now = SystemTime::now();
        assert_eq!(held_for(None, now, gap), None);
        let recent = now - Duration::from_secs(100);
        assert_eq!(
            held_for(Some(recent), now, gap),
            Some(Duration::from_secs(500))
        );
        let old = now - Duration::from_secs(601);
        assert_eq!(held_for(Some(old), now, gap), None);
        // A clock that went back holds for the whole gap, not for ever.
        let ahead = now + Duration::from_secs(5);
        assert_eq!(held_for(Some(ahead), now, gap), Some(gap));
    }

    #[test]
    fn two_replaced_stores_are_kept_and_the_time_is_read_from_the_name() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(last_replaced(dir.path()), None);
        let day = |d: u32| {
            chrono::NaiveDate::from_ymd_opt(2026, 11, d)
                .unwrap()
                .and_hms_opt(10, 0, 0)
                .unwrap()
                .and_utc()
        };
        for d in [3, 1, 2] {
            std::fs::write(aside_name(dir.path(), day(d)), b"x").unwrap();
        }
        // Same second again: a second name, not an overwrite.
        let twin = aside_name(dir.path(), day(3));
        assert!(twin.to_string_lossy().ends_with("-2"), "{twin:?}");
        std::fs::write(&twin, b"x").unwrap();
        std::fs::write(dir.path().join("state.redb.before-restore-x"), b"x").unwrap();
        assert_eq!(last_replaced(dir.path()), Some(SystemTime::from(day(3))));

        prune_asides(dir.path());
        let mut left: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(
            left,
            [
                "state.redb.before-restore-x",
                "state.redb.replaced-20261103T100000Z",
                "state.redb.replaced-20261103T100000Z-2",
            ]
        );
    }

    #[test]
    fn a_cut_short_replacement_is_finished_at_the_next_start() {
        let dir = tempfile::tempdir().unwrap();
        let live = dir.path().join("state.redb");
        let incoming = dir.path().join(RESYNC_FILE);
        // Beside a store it is a leftover.
        std::fs::write(&live, b"old").unwrap();
        std::fs::write(&incoming, b"new").unwrap();
        finish_interrupted(dir.path());
        assert!(!incoming.exists());
        assert_eq!(std::fs::read(&live).unwrap(), b"old");
        // With no store it is the store.
        std::fs::rename(&live, dir.path().join("state.redb.replaced-x")).unwrap();
        std::fs::write(&incoming, b"new").unwrap();
        finish_interrupted(dir.path());
        assert!(!incoming.exists());
        assert_eq!(std::fs::read(&live).unwrap(), b"new");
    }
}
