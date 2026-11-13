// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Results of the commands this agent has completed, kept for a while.
//!
//! When the leader changes, the new one issues again what the old one had
//! outstanding, under the same ids. A command this agent already ran must
//! not run a second time, and its result must still reach the control
//! plane: the agent finds the id here and answers with what it kept.
//!
//! The record is a small file in the data directory, so it is still there
//! after the agent restarts. It holds at most [`KEEP_MAX`] results and none
//! older than [`KEEP_FOR`]. A host without a writable data directory keeps
//! the record in memory only.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chrono::{DateTime, Utc};
use pistack_rpc::proto::{CommandId, CommandResult};
use serde::{Deserialize, Serialize};

/// How long a result is kept. The control plane gives up on a command long
/// before this, so an id that can still be delivered is still known.
pub const KEEP_FOR: std::time::Duration = std::time::Duration::from_secs(3600);
/// The most results kept. The oldest go first.
pub const KEEP_MAX: usize = 256;

/// File name under the agent's data directory.
pub const FILE: &str = "commands-done.json";

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Entry {
    at: DateTime<Utc>,
    result: CommandResult,
}

pub struct Completed {
    path: Option<PathBuf>,
    entries: Mutex<Vec<Entry>>,
}

impl Completed {
    /// Read the record at `path`. A missing or unreadable file is an empty
    /// record.
    pub fn open(path: impl AsRef<Path>) -> Self {
        let path = path.as_ref().to_owned();
        let mut entries: Vec<Entry> = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        prune(&mut entries, Utc::now());
        Self {
            path: Some(path),
            entries: Mutex::new(entries),
        }
    }

    pub fn in_memory() -> Self {
        Self {
            path: None,
            entries: Mutex::new(Vec::new()),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<Entry>> {
        self.entries.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The result kept for `id`, when the command was completed here and
    /// not too long ago.
    pub fn get(&self, id: CommandId) -> Option<CommandResult> {
        let oldest = Utc::now() - KEEP_FOR;
        self.lock()
            .iter()
            .find(|e| e.result.id == id && e.at >= oldest)
            .map(|e| e.result.clone())
    }

    /// Keep `result`, and write the record out.
    pub fn record(&self, result: &CommandResult) {
        self.record_at(result, Utc::now());
    }

    fn record_at(&self, result: &CommandResult, at: DateTime<Utc>) {
        let mut entries = self.lock();
        entries.retain(|e| e.result.id != result.id);
        entries.push(Entry {
            at,
            result: result.clone(),
        });
        prune(&mut entries, at);
        let Some(path) = &self.path else {
            return;
        };
        // Written whole and renamed into place, so a crash leaves the old
        // file or the new one.
        let tmp = path.with_extension("json.tmp");
        let written = serde_json::to_vec(&*entries)
            .map_err(std::io::Error::other)
            .and_then(|bytes| std::fs::write(&tmp, bytes))
            .and_then(|()| std::fs::rename(&tmp, path));
        if let Err(e) = written {
            tracing::debug!(path = %path.display(), error = %e, "anvil: completed commands were not written");
        }
    }
}

fn prune(entries: &mut Vec<Entry>, now: DateTime<Utc>) {
    let oldest = now - KEEP_FOR;
    entries.retain(|e| e.at >= oldest);
    if entries.len() > KEEP_MAX {
        let extra = entries.len() - KEEP_MAX;
        entries.drain(..extra);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn a_result_is_still_known_after_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE);
        let id = Uuid::now_v7();
        let done = Completed::open(&path);
        assert!(done.get(id).is_none());
        done.record(&CommandResult::ok(id).with_payload(serde_json::json!({"device": "vdb"})));
        drop(done);

        let again = Completed::open(&path);
        let kept = again.get(id).unwrap();
        assert!(kept.ok);
        assert_eq!(kept.payload["device"], "vdb");
        assert!(again.get(Uuid::now_v7()).is_none());
    }

    #[test]
    fn old_results_and_results_past_the_limit_are_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE);
        let done = Completed::open(&path);
        let old = Uuid::now_v7();
        done.record_at(
            &CommandResult::ok(old),
            Utc::now() - KEEP_FOR - chrono::Duration::seconds(1),
        );
        assert!(done.get(old).is_none());

        let first = Uuid::now_v7();
        done.record(&CommandResult::ok(first));
        let mut last = first;
        for _ in 0..KEEP_MAX {
            last = Uuid::now_v7();
            done.record(&CommandResult::ok(last));
        }
        assert!(done.get(first).is_none());
        assert!(done.get(last).is_some());
        assert_eq!(done.lock().len(), KEEP_MAX);
        // The file holds the same, and no more.
        let again = Completed::open(&path);
        assert_eq!(again.lock().len(), KEEP_MAX);
        assert!(again.get(old).is_none());
    }

    #[test]
    fn a_directory_that_is_not_there_keeps_the_record_in_memory() {
        let done = Completed::open("/nonexistent-pistack-dir/commands-done.json");
        let id = Uuid::now_v7();
        done.record(&CommandResult::failed(id, "no such image"));
        assert_eq!(done.get(id).unwrap().message, "no such image");
    }
}
