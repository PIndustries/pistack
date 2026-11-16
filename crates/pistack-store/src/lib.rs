// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The PiStack state store.
//!
//! One embedded, transactional KV (redb) holds every resource of every
//! module, keyed `{kind}/{id}`. Modules use the typed `Collection<K>` API and
//! never touch bytes. The `Backend` trait is the seam where `openraft`
//! replication lands later: same API, replicated log underneath.
//!
//! Values are JSON. Human-inspectable with `pistack store dump`. Most
//! changes to what is stored are a serde default. One that older code would
//! misread raises [`SCHEMA_VERSION`] and adds a step to `MIGRATIONS`.

use chrono::{DateTime, Utc};
use parking_lot::RwLock;
use pistack_types::{ApiError, Event, EventAction, Id, Kind, Meta};
use redb::{Database, ReadableTable, TableDefinition};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use tokio::sync::broadcast;

const RESOURCES: TableDefinition<&str, &[u8]> = TableDefinition::new("resources");
/// Secondary index: `{kind}/{project}/{name}` -> id, enforcing unique names
/// per project per kind and giving fast lookup by name.
const NAMES: TableDefinition<&str, &str> = TableDefinition::new("names");
/// Misc small values (cluster id, key material refs, counters).
const META: TableDefinition<&str, &[u8]> = TableDefinition::new("meta");

/// Layout version of the store this binary reads and writes. The store
/// keeps the version it was last written at in `meta`, and every open
/// compares the two. Raise this by one, and add the step to `MIGRATIONS`,
/// when a release changes what is stored in a way older code would misread.
/// A store with no marker predates the marker and is version 1.
pub const SCHEMA_VERSION: u32 = 1;
const SCHEMA_KEY: &str = "schema_version";

/// One step up: `run` turns a store at `to - 1` into one at `to`. It runs in
/// a write transaction that also records `to`, so a step is applied whole
/// or not at all, and never twice.
struct Migration {
    to: u32,
    run: fn(&redb::WriteTransaction) -> Result<()>,
}

/// Every step from version 2 up to [`SCHEMA_VERSION`]. None yet.
const MIGRATIONS: &[Migration] = &[];

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("conflict: {0}")]
    Conflict(String),
    #[error("revision mismatch: expected {expected}, found {found}")]
    Revision { expected: u64, found: u64 },
    /// This node is not the leader. Ordinary writes are refused; follower
    /// apply and cluster bookkeeping still go through.
    #[error("not the leader")]
    NotLeader,
    /// The store was written by a newer binary. Nothing was changed.
    #[error(
        "the store is at schema version {found} and this pistack reads up to {supported}; \
         run a pistack at least as new as the one that last opened this store"
    )]
    SchemaTooNew { found: u32, supported: u32 },
    #[error("storage: {0}")]
    Storage(String),
    #[error("codec: {0}")]
    Codec(#[from] serde_json::Error),
}

impl StoreError {
    /// True when the write fence refused this write after stand-down.
    pub fn is_not_leader(&self) -> bool {
        matches!(self, Self::NotLeader)
    }
}

impl From<redb::Error> for StoreError {
    fn from(e: redb::Error) -> Self {
        StoreError::Storage(e.to_string())
    }
}
macro_rules! redb_err {
    ($($t:ty),*) => {$(
        impl From<$t> for StoreError {
            fn from(e: $t) -> Self { StoreError::Storage(e.to_string()) }
        }
    )*};
}
redb_err!(
    redb::DatabaseError,
    redb::TransactionError,
    redb::TableError,
    redb::StorageError,
    redb::CommitError
);

impl From<StoreError> for ApiError {
    fn from(e: StoreError) -> Self {
        match e {
            StoreError::NotFound(m) => ApiError::not_found(m),
            StoreError::Conflict(m) => ApiError::conflict(m),
            StoreError::Revision { expected, found } => ApiError::conflict(format!(
                "resource changed: expected revision {expected}, found {found}"
            )),
            StoreError::NotLeader => ApiError::conflict("not the leader"),
            e @ StoreError::SchemaTooNew { .. } => ApiError::internal(e.to_string()),
            StoreError::Storage(m) => ApiError::internal(m),
            StoreError::Codec(e) => ApiError::internal(e.to_string()),
        }
    }
}

pub type Result<T> = std::result::Result<T, StoreError>;

/// Backend seam. Today: local redb. Tomorrow: a Raft state machine that
/// applies the same operations after consensus.
pub trait Backend: Send + Sync + 'static {
    fn get(&self, table: &str, key: &str) -> Result<Option<Vec<u8>>>;
    fn put(&self, table: &str, key: &str, value: &[u8]) -> Result<()>;
    fn delete(&self, table: &str, key: &str) -> Result<bool>;
    /// All entries whose key starts with `prefix`, in key order.
    fn scan(&self, table: &str, prefix: &str) -> Result<Vec<(String, Vec<u8>)>>;
    /// Atomic multi-op write.
    fn batch(&self, ops: Vec<Op>) -> Result<()>;
    fn set_record_log(&self, on: bool);
    fn set_log_term(&self, term: u64);
    /// When set, ordinary writes fail with [`StoreError::NotLeader`]. Cleared
    /// when this node becomes leader; set when it stands down. Apply and
    /// follower bookkeeping bypass it.
    fn set_write_fence(&self, on: bool);
    /// True while [`Store::apply_entries`] is copying the leader's log.
    fn set_applying(&self, on: bool);
    fn read_log(&self, after: u64, limit: usize) -> Result<Vec<LogEntry>>;
    /// Current record flag. `apply_entries` turns recording off, then restores this.
    fn record_log(&self) -> bool;
    /// The store as it stands, read once, for a follower checking a whole page
    /// of the leader's rows before it writes any of them. A backend built on a
    /// redb file hands back its read transaction; any other is asked row by row
    /// through [`Backend::get`] instead.
    fn whole_store_read(&self) -> Option<redb::ReadTransaction> {
        None
    }
    /// Write a consistent copy of the whole store to `dst`. `src` is the
    /// file this backend was opened from. The default copies the file; the
    /// redb backend writes a fresh, compact database from one read
    /// transaction instead, so writers are never blocked and the copy never
    /// holds a half-written commit.
    fn snapshot_to(&self, src: &Path, dst: &Path) -> Result<()> {
        std::fs::copy(src, dst).map_err(|e| StoreError::Storage(e.to_string()))?;
        Ok(())
    }
    /// Row counts and value bytes per table, for `pistack store stats`.
    fn stats(&self) -> Result<Vec<TableStats>> {
        Ok(Vec::new())
    }
}

/// Size of one table, or of the replication log inside `meta`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TableStats {
    pub table: String,
    pub rows: u64,
    pub key_bytes: u64,
    pub value_bytes: u64,
}

#[derive(Debug, Clone)]
pub enum Op {
    Put {
        table: String,
        key: String,
        value: Vec<u8>,
    },
    Delete {
        table: String,
        key: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogOp {
    pub table: String,
    pub key: String,
    pub value: Option<Vec<u8>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogEntry {
    pub index: u64,
    pub term: u64,
    pub ops: Vec<LogOp>,
}

/// Meta keys written by the log itself, or by cluster bookkeeping. They are
/// stored, but they are not log entries and do not advance the commit index.
fn is_bookkeeping(op: &Op) -> bool {
    let (table, key) = match op {
        Op::Put { table, key, .. } | Op::Delete { table, key } => (table.as_str(), key.as_str()),
    };
    table == "meta"
        && (key == "cluster_commit"
            || key.starts_with("cluster_log/")
            || matches!(
                key,
                "cluster_term"
                    | "cluster_voted_for"
                    | "cluster_leader"
                    | "node_id"
                    | LOG_START_KEY
                    | SCHEMA_KEY
            ))
}

/// Membership keys the follow loop writes after stepping down. When recording
/// is on they still go in the log (a leader's `/cluster/join` must); when the
/// write fence is up they are allowed without logging.
fn is_follower_meta(op: &Op) -> bool {
    let (table, key) = match op {
        Op::Put { table, key, .. } | Op::Delete { table, key } => (table.as_str(), key.as_str()),
    };
    table == "meta" && matches!(key, "members" | "cluster_leader_id")
}

fn allowed_when_fenced(op: &Op) -> bool {
    is_bookkeeping(op) || is_follower_meta(op)
}

/// The store as it stands, read one page of the leader's rows against it.
enum Read<'a> {
    /// One consistent read of the whole store, opened once for the page.
    Tx(&'a redb::ReadTransaction),
    /// No whole-store read to hand out: the backend is asked a row at a time.
    Rows(&'a Arc<dyn Backend>),
}

impl Read<'_> {
    /// Whether one row is in the store, and carried out as an action because
    /// that is all [`known`] can report before the page says what stood.
    fn stands(&self, table: &str, key: &str) -> Result<bool> {
        match self {
            Read::Tx(tx) => {
                if table == "names" {
                    let names = tx.open_table(NAMES)?;
                    return Ok(names.get(key)?.is_some());
                }
                let res = tx.open_table(RESOURCES)?;
                Ok(res.get(key)?.is_some())
            }
            Read::Rows(backend) => backend.get(table, key).map(|v| v.is_some()),
        }
    }
}

/// What one logged row write names, and whether the row it writes stood before
/// it — or `None` when the write is not of a row anybody watches: a `meta` row
/// (the log's own, a counter, cluster bookkeeping), the gateway's audit trail, a
/// name index entry, or a row this binary cannot read as one.
fn known(read: &Read<'_>, op: &LogOp) -> Result<Option<Change>> {
    let (table, key, value) = (op.table.as_str(), op.key.as_str(), op.value.as_deref());
    // Only a `resources` row names something a watcher watches. A `meta` row is
    // the log's own, a counter's, or cluster bookkeeping; a `names` row is the
    // name index, which points at a resource row that is announced in its own
    // right. Neither is worth a read of the store to answer `None`, so none is
    // read before this returns.
    if table != "resources" {
        return Ok(None);
    }
    let stood = read.stands(table, key)?;
    // The row key is `{kind}/{id}`: the head names the kind, the tail is the
    // row's address. Reading the kind off the key rather than out of the row is
    // what keeps a row whose name looks like an id of another kind from being
    // announced under the wrong one.
    let Some((kind_, rest)) = key.split_once('/') else {
        return Ok(None);
    };
    let Some(id) = Id::parse(intern(kind_), rest) else {
        return Ok(None);
    };
    // The audit trail is a row like any other, but it is bookkeeping: the
    // gateway writes one for every call that changes something, and a follower
    // that announced each would flood every watcher on this node.
    if kind_ == AUDIT_KIND {
        return Ok(None);
    }
    let Some(bytes) = value else {
        // A row that is gone says nothing about itself: what it was is not
        // needed, since a watcher is told to look again, not what to look at.
        return Ok(Some(Change {
            key: key.to_owned(),
            kind: kind_.to_owned(),
            id,
            project: None,
            revision: 0,
            at: None,
            stood,
            action: EventAction::Updated,
        }));
    };
    // A row of a shape this binary cannot parse still lands in the store; it
    // simply has no metadata to name itself with, so nothing is said about it.
    let Ok(meta) = serde_json::from_slice::<Meta>(bytes) else {
        return Ok(None);
    };
    Ok(Some(Change {
        key: key.to_owned(),
        kind: kind_.to_owned(),
        id,
        project: meta.project,
        revision: meta.revision,
        at: Some(meta.updated_at),
        stood,
        action: EventAction::Updated,
    }))
}

/// One row change a follower applied, and what to announce once it is on disk.
/// `action` is settled by [`Store::plan`], which is the only thing that knows
/// what stood before the write; `known` leaves it a placeholder.
#[derive(Clone)]
struct Change {
    /// The row's key, `{kind}/{id}`: how a second write of the row in the same
    /// page finds the name the first write read out of it.
    key: String,
    kind: String,
    id: Id,
    project: Option<Id>,
    revision: u64,
    at: Option<DateTime<Utc>>,
    /// Whether the row stood in the store when this write reached it. Only the
    /// page's first write of a row can be asked of the store; `plan` settles the
    /// rest from what its own earlier writes left.
    stood: bool,
    action: EventAction,
}

impl Change {
    fn into_event(self) -> Event {
        Event {
            kind: self.kind,
            id: self.id,
            project: self.project,
            // The row's own stamp when the row still says what it is; this
            // moment when it does not, which is the case of a row just removed.
            at: self.at.unwrap_or_else(Utc::now),
            action: self.action,
            revision: self.revision,
        }
    }
}

/// What a write to a row that stood, or did not, amounts to.
fn action_for(stood: bool, puts: bool) -> EventAction {
    match (stood, puts) {
        (false, true) => EventAction::Created,
        (true, true) => EventAction::Updated,
        (true, false) => EventAction::Deleted,
        // A delete of a row that was not there is never announced: it changed
        // nothing, and a watcher told about it would look and find what was
        // already there.
        (false, false) => EventAction::Deleted,
    }
}

fn log_key(index: u64) -> String {
    format!("cluster_log/{index:020}")
}

/// First log row this store still keeps. Rows below it were trimmed by
/// [`Store::compact_log`]; the state they produced is in the tables.
const LOG_START_KEY: &str = "cluster_log_start";

/// The kind the gateway files its audit trail under. The store does not know
/// the resource kinds — modules name their own — so this is the one exception
/// it names.
const AUDIT_KIND: &str = "audit";

/// Events sent to overflow the bus. The server's own channel is this wide and
/// the store never grows it; see [`Store::announce_all_changed`].
const ANNOUNCE_ALL_FILL: usize = 4096 + 1;

/// Rows per write transaction when copying a store or trimming its log, so
/// neither holds one huge transaction in memory.
const COPY_BATCH_ROWS: usize = 2_000;
const COPY_BATCH_BYTES: usize = 8 << 20;

fn log_op_from(op: &Op) -> LogOp {
    match op {
        Op::Put { table, key, value } => LogOp {
            table: table.clone(),
            key: key.clone(),
            value: Some(value.clone()),
        },
        Op::Delete { table, key } => LogOp {
            table: table.clone(),
            key: key.clone(),
            value: None,
        },
    }
}

fn op_from_log(op: &LogOp) -> Op {
    match &op.value {
        Some(value) => Op::Put {
            table: op.table.clone(),
            key: op.key.clone(),
            value: value.clone(),
        },
        None => Op::Delete {
            table: op.table.clone(),
            key: op.key.clone(),
        },
    }
}

/// redb-backed `Backend`.
pub struct RedbBackend {
    db: Database,
    record: AtomicBool,
    term: AtomicU64,
    /// Set on stand-down; cleared on become-leader. See [`Backend::set_write_fence`].
    fenced: AtomicBool,
    /// Set for the duration of [`Store::apply_entries`].
    applying: AtomicBool,
    /// Holds across fence flag changes and the fence check in [`Backend::batch`].
    gate: parking_lot::Mutex<()>,
}

impl RedbBackend {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::open_at(path, SCHEMA_VERSION, MIGRATIONS)
    }

    /// Open as a binary whose schema version is `target` and whose steps are
    /// `steps`. Tests use this to stand in for a newer binary.
    fn open_at(path: impl AsRef<Path>, target: u32, steps: &[Migration]) -> Result<Self> {
        let db = Database::create(path)?;
        settle_schema(&db, target, steps)?;
        Ok(Self::around(db))
    }

    pub fn in_memory() -> Result<Self> {
        let db = Database::builder().create_with_backend(redb::backends::InMemoryBackend::new())?;
        settle_schema(&db, SCHEMA_VERSION, MIGRATIONS)?;
        Ok(Self::around(db))
    }

    fn around(db: Database) -> Self {
        Self {
            db,
            record: AtomicBool::new(false),
            term: AtomicU64::new(0),
            fenced: AtomicBool::new(false),
            applying: AtomicBool::new(false),
            gate: parking_lot::Mutex::new(()),
        }
    }

    fn table_def(name: &str) -> Result<TableDefinition<'static, &'static str, &'static [u8]>> {
        match name {
            "resources" => Ok(RESOURCES),
            "meta" => Ok(META),
            other => Err(StoreError::Storage(format!("unknown table {other}"))),
        }
    }
}

/// Check the store's schema version against `target`, make sure the tables
/// and the marker exist, and run the steps a newer binary brings. A store
/// that is newer than `target` is refused before anything is written to it.
fn settle_schema(db: &Database, target: u32, steps: &[Migration]) -> Result<()> {
    let tx = db.begin_write()?;
    let found = {
        let meta = tx.open_table(META)?;
        let raw = meta.get(SCHEMA_KEY)?;
        match raw {
            Some(raw) => Some(serde_json::from_slice::<u32>(raw.value())?),
            None => None,
        }
    };
    if let Some(found) = found.filter(|found| *found > target) {
        tx.abort()?;
        return Err(StoreError::SchemaTooNew {
            found,
            supported: target,
        });
    }
    {
        tx.open_table(RESOURCES)?;
        tx.open_table(NAMES)?;
        if found.is_none() {
            let mut meta = tx.open_table(META)?;
            meta.insert(SCHEMA_KEY, serde_json::to_vec(&1u32)?.as_slice())?;
        }
    }
    tx.commit()?;
    let mut at = found.unwrap_or(1);
    while at < target {
        let next = at + 1;
        let step = steps.iter().find(|step| step.to == next).ok_or_else(|| {
            StoreError::Storage(format!(
                "the store is at schema version {at} and this pistack has no step to version {next}"
            ))
        })?;
        // Dropping the transaction on an error undoes the step.
        let tx = db.begin_write()?;
        (step.run)(&tx).map_err(|err| {
            StoreError::Storage(format!(
                "migrating the store from schema version {at} to {next}: {err}"
            ))
        })?;
        {
            let mut meta = tx.open_table(META)?;
            meta.insert(SCHEMA_KEY, serde_json::to_vec(&next)?.as_slice())?;
        }
        tx.commit()?;
        tracing::info!(from = at, to = next, "store schema migrated");
        at = next;
    }
    Ok(())
}

impl Backend for RedbBackend {
    fn get(&self, table: &str, key: &str) -> Result<Option<Vec<u8>>> {
        let tx = self.db.begin_read()?;
        if table == "names" {
            let t = tx.open_table(NAMES)?;
            return Ok(t.get(key)?.map(|v| v.value().as_bytes().to_vec()));
        }
        let t = tx.open_table(Self::table_def(table)?)?;
        Ok(t.get(key)?.map(|v| v.value().to_vec()))
    }

    fn put(&self, table: &str, key: &str, value: &[u8]) -> Result<()> {
        self.batch(vec![Op::Put {
            table: table.into(),
            key: key.into(),
            value: value.to_vec(),
        }])
    }

    fn delete(&self, table: &str, key: &str) -> Result<bool> {
        let existed = self.get(table, key)?.is_some();
        self.batch(vec![Op::Delete {
            table: table.into(),
            key: key.into(),
        }])?;
        Ok(existed)
    }

    fn scan(&self, table: &str, prefix: &str) -> Result<Vec<(String, Vec<u8>)>> {
        let tx = self.db.begin_read()?;
        let mut out = Vec::new();
        if table == "names" {
            let t = tx.open_table(NAMES)?;
            for item in t.range(prefix..)? {
                let (k, v) = item?;
                let k = k.value();
                if !k.starts_with(prefix) {
                    break;
                }
                out.push((k.to_owned(), v.value().as_bytes().to_vec()));
            }
            return Ok(out);
        }
        let t = tx.open_table(Self::table_def(table)?)?;
        for item in t.range(prefix..)? {
            let (k, v) = item?;
            let k = k.value();
            if !k.starts_with(prefix) {
                break;
            }
            out.push((k.to_owned(), v.value().to_vec()));
        }
        Ok(out)
    }

    fn batch(&self, ops: Vec<Op>) -> Result<()> {
        let _gate = self.gate.lock();
        let recording = self.record.load(Ordering::Relaxed);
        let applying = self.applying.load(Ordering::Relaxed);
        let fenced = self.fenced.load(Ordering::Relaxed);
        if fenced && !applying && ops.iter().any(|op| !allowed_when_fenced(op)) {
            return Err(StoreError::NotLeader);
        }
        let tx = self.db.begin_write()?;
        {
            let mut res = tx.open_table(RESOURCES)?;
            let mut names = tx.open_table(NAMES)?;
            let mut meta = tx.open_table(META)?;
            // One log row for the non-bookkeeping ops, in this same transaction.
            let logged: Vec<LogOp> = if recording {
                ops.iter()
                    .filter(|op| !is_bookkeeping(op))
                    .map(log_op_from)
                    .collect()
            } else {
                Vec::new()
            };
            let next = if logged.is_empty() {
                None
            } else {
                let current = {
                    match meta.get("cluster_commit")? {
                        Some(raw) => serde_json::from_slice::<u64>(raw.value())?,
                        None => 0,
                    }
                };
                Some(current + 1)
            };
            for op in ops {
                match op {
                    Op::Put { table, key, value } => match table.as_str() {
                        "resources" => {
                            res.insert(key.as_str(), value.as_slice())?;
                        }
                        "meta" => {
                            meta.insert(key.as_str(), value.as_slice())?;
                        }
                        "names" => {
                            let v = std::str::from_utf8(&value)
                                .map_err(|e| StoreError::Storage(e.to_string()))?;
                            names.insert(key.as_str(), v)?;
                        }
                        other => return Err(StoreError::Storage(format!("unknown table {other}"))),
                    },
                    Op::Delete { table, key } => match table.as_str() {
                        "resources" => {
                            res.remove(key.as_str())?;
                        }
                        "meta" => {
                            meta.remove(key.as_str())?;
                        }
                        "names" => {
                            names.remove(key.as_str())?;
                        }
                        other => return Err(StoreError::Storage(format!("unknown table {other}"))),
                    },
                }
            }
            if let Some(index) = next {
                let entry = LogEntry {
                    index,
                    term: self.term.load(Ordering::Acquire),
                    ops: logged,
                };
                let raw = serde_json::to_vec(&entry)?;
                let key = log_key(index);
                meta.insert(key.as_str(), raw.as_slice())?;
                let commit = serde_json::to_vec(&index)?;
                meta.insert("cluster_commit", commit.as_slice())?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    fn set_record_log(&self, on: bool) {
        let _gate = self.gate.lock();
        self.record.store(on, Ordering::Relaxed);
    }

    fn set_log_term(&self, term: u64) {
        self.term.store(term, Ordering::Release);
    }

    fn set_write_fence(&self, on: bool) {
        let _gate = self.gate.lock();
        self.fenced.store(on, Ordering::Relaxed);
    }

    fn set_applying(&self, on: bool) {
        let _gate = self.gate.lock();
        self.applying.store(on, Ordering::Relaxed);
    }

    /// Reads only the rows it returns: a range from `after + 1`, not a scan
    /// of the whole log.
    fn read_log(&self, after: u64, limit: usize) -> Result<Vec<LogEntry>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let tx = self.db.begin_read()?;
        let meta = tx.open_table(META)?;
        let start = log_key(after.saturating_add(1));
        let mut entries = Vec::new();
        for item in meta.range(start.as_str()..)? {
            let (key, raw) = item?;
            if !key.value().starts_with("cluster_log/") {
                break;
            }
            entries.push(serde_json::from_slice::<LogEntry>(raw.value())?);
            if entries.len() >= limit {
                break;
            }
        }
        Ok(entries)
    }

    fn record_log(&self) -> bool {
        self.record.load(Ordering::Acquire)
    }

    fn whole_store_read(&self) -> Option<redb::ReadTransaction> {
        self.db.begin_read().ok()
    }

    fn snapshot_to(&self, _src: &Path, dst: &Path) -> Result<()> {
        let read = self.db.begin_read()?;
        let out = Database::create(dst)?;
        {
            let tx = out.begin_write()?;
            tx.open_table(RESOURCES)?;
            tx.open_table(NAMES)?;
            tx.open_table(META)?;
            tx.commit()?;
        }
        for def in [RESOURCES, META] {
            let table = read.open_table(def)?;
            let mut rows: Vec<(String, Vec<u8>)> = Vec::new();
            let mut bytes = 0usize;
            for item in table.iter()? {
                let (key, value) = item?;
                bytes += key.value().len() + value.value().len();
                rows.push((key.value().to_owned(), value.value().to_vec()));
                if rows.len() >= COPY_BATCH_ROWS || bytes >= COPY_BATCH_BYTES {
                    write_rows(&out, def, &rows)?;
                    rows.clear();
                    bytes = 0;
                }
            }
            write_rows(&out, def, &rows)?;
        }
        let names = read.open_table(NAMES)?;
        let mut rows: Vec<(String, String)> = Vec::new();
        for item in names.iter()? {
            let (key, value) = item?;
            rows.push((key.value().to_owned(), value.value().to_owned()));
            if rows.len() >= COPY_BATCH_ROWS {
                write_names(&out, &rows)?;
                rows.clear();
            }
        }
        write_names(&out, &rows)?;
        Ok(())
    }

    fn stats(&self) -> Result<Vec<TableStats>> {
        let tx = self.db.begin_read()?;
        let mut out = Vec::new();
        let mut log = TableStats {
            table: "meta:cluster_log".into(),
            ..TableStats::default()
        };
        for (name, def) in [("resources", RESOURCES), ("meta", META)] {
            let mut stats = TableStats {
                table: name.into(),
                ..TableStats::default()
            };
            for item in tx.open_table(def)?.iter()? {
                let (key, value) = item?;
                let (k, v) = (key.value().len() as u64, value.value().len() as u64);
                stats.rows += 1;
                stats.key_bytes += k;
                stats.value_bytes += v;
                if key.value().starts_with("cluster_log/") {
                    log.rows += 1;
                    log.key_bytes += k;
                    log.value_bytes += v;
                }
            }
            out.push(stats);
        }
        let mut names = TableStats {
            table: "names".into(),
            ..TableStats::default()
        };
        for item in tx.open_table(NAMES)?.iter()? {
            let (key, value) = item?;
            names.rows += 1;
            names.key_bytes += key.value().len() as u64;
            names.value_bytes += value.value().len() as u64;
        }
        out.push(names);
        out.push(log);
        Ok(out)
    }
}

fn write_rows(
    db: &Database,
    def: TableDefinition<&str, &[u8]>,
    rows: &[(String, Vec<u8>)],
) -> Result<()> {
    if rows.is_empty() {
        return Ok(());
    }
    let tx = db.begin_write()?;
    {
        let mut table = tx.open_table(def)?;
        for (key, value) in rows {
            table.insert(key.as_str(), value.as_slice())?;
        }
    }
    tx.commit()?;
    Ok(())
}

fn write_names(db: &Database, rows: &[(String, String)]) -> Result<()> {
    if rows.is_empty() {
        return Ok(());
    }
    let tx = db.begin_write()?;
    {
        let mut table = tx.open_table(NAMES)?;
        for (key, value) in rows {
            table.insert(key.as_str(), value.as_str())?;
        }
    }
    tx.commit()?;
    Ok(())
}

/// The store handle modules hold. Clone freely.
#[derive(Clone)]
pub struct Store {
    backend: Arc<dyn Backend>,
    events: broadcast::Sender<Event>,
    /// Per-process write lock so read-modify-write on one key is serialised.
    /// Coarse on purpose: correctness first, sharding later.
    write_lock: Arc<RwLock<()>>,
}

impl Store {
    pub fn new(backend: Arc<dyn Backend>, events: broadcast::Sender<Event>) -> Self {
        Self {
            backend,
            events,
            write_lock: Arc::new(RwLock::new(())),
        }
    }

    pub fn open(path: impl AsRef<Path>, events: broadcast::Sender<Event>) -> Result<Self> {
        Ok(Self::new(Arc::new(RedbBackend::open(path)?), events))
    }

    pub fn in_memory() -> Result<Self> {
        let (tx, _rx) = broadcast::channel(1024);
        Ok(Self::new(Arc::new(RedbBackend::in_memory()?), tx))
    }

    pub fn events(&self) -> broadcast::Sender<Event> {
        self.events.clone()
    }

    /// Raw prefix scan, for `pistack store dump` and tests.
    pub fn raw_scan(&self, table: &str, prefix: &str) -> Result<Vec<(String, Vec<u8>)>> {
        self.backend.scan(table, prefix)
    }

    pub fn collection<K: Kind>(&self) -> Collection<K> {
        Collection {
            store: self.clone(),
            _k: std::marker::PhantomData,
        }
    }

    // ---- small typed values -------------------------------------------------

    pub fn get_meta<T: DeserializeOwned>(&self, key: &str) -> Result<Option<T>> {
        Ok(match self.backend.get("meta", key)? {
            Some(b) => Some(serde_json::from_slice(&b)?),
            None => None,
        })
    }

    pub fn put_meta<T: Serialize>(&self, key: &str, value: &T) -> Result<()> {
        self.backend.put("meta", key, &serde_json::to_vec(value)?)
    }

    /// Put `put_key` and delete `delete_key` in one transaction: after a
    /// crash the store holds either both old values or both new ones.
    pub fn put_and_delete_meta<T: Serialize>(
        &self,
        put_key: &str,
        value: &T,
        delete_key: &str,
    ) -> Result<()> {
        self.backend.batch(vec![
            Op::Put {
                table: "meta".into(),
                key: put_key.into(),
                value: serde_json::to_vec(value)?,
            },
            Op::Delete {
                table: "meta".into(),
                key: delete_key.into(),
            },
        ])
    }

    /// Put two meta keys in one transaction, and so in one log entry: a
    /// follower applies both or neither.
    pub fn put_meta_pair<A: Serialize, B: Serialize>(
        &self,
        first: (&str, &A),
        second: (&str, &B),
    ) -> Result<()> {
        self.backend.batch(vec![
            Op::Put {
                table: "meta".into(),
                key: first.0.into(),
                value: serde_json::to_vec(first.1)?,
            },
            Op::Put {
                table: "meta".into(),
                key: second.0.into(),
                value: serde_json::to_vec(second.1)?,
            },
        ])
    }

    /// Delete one meta key.
    pub fn delete_meta(&self, key: &str) -> Result<bool> {
        self.backend.delete("meta", key)
    }

    /// Delete several meta keys in one transaction, and so in one log entry
    /// and one sync. A key that is not there is skipped.
    pub fn delete_meta_many(&self, keys: &[String]) -> Result<()> {
        if keys.is_empty() {
            return Ok(());
        }
        self.backend.batch(
            keys.iter()
                .map(|key| Op::Delete {
                    table: "meta".into(),
                    key: key.clone(),
                })
                .collect(),
        )
    }

    pub fn set_record_log(&self, on: bool) {
        self.backend.set_record_log(on);
    }

    pub fn set_log_term(&self, term: u64) {
        self.backend.set_log_term(term);
    }

    /// Refuse ordinary writes until [`Self::set_write_fence`] is cleared.
    /// Cluster stand-down sets this; become-leader clears it.
    pub fn set_write_fence(&self, on: bool) {
        self.backend.set_write_fence(on);
    }

    pub fn read_log(&self, after: u64, limit: usize) -> Result<Vec<LogEntry>> {
        self.backend.read_log(after, limit)
    }

    pub fn commit_index(&self) -> Result<u64> {
        Ok(self.get_meta::<u64>("cluster_commit")?.unwrap_or(0))
    }

    /// First log row still kept. 1 until [`Self::compact_log`] trims.
    pub fn log_start(&self) -> Result<u64> {
        Ok(self.get_meta::<u64>(LOG_START_KEY)?.unwrap_or(1).max(1))
    }

    /// Trim log rows below `upto`, at most `max_rows` of them, and return how
    /// many were removed. The commit row is never removed, so votes and
    /// follower tip checks still see its term. The state those rows produced
    /// stays in the tables and in every snapshot; a follower that still
    /// needs a trimmed row is told to rejoin from a snapshot.
    ///
    /// Trimming is bookkeeping: it is never logged, and the write fence does
    /// not refuse it.
    pub fn compact_log(&self, upto: u64, max_rows: u64) -> Result<u64> {
        let commit = self.commit_index()?;
        let upto = upto.min(commit);
        let start = self.log_start()?;
        if upto <= start {
            return Ok(0);
        }
        let end = upto.min(start.saturating_add(max_rows));
        let mut at = start;
        while at < end {
            let stop = end.min(at + COPY_BATCH_ROWS as u64);
            let mut ops: Vec<Op> = (at..stop)
                .map(|index| Op::Delete {
                    table: "meta".into(),
                    key: log_key(index),
                })
                .collect();
            ops.push(Op::Put {
                table: "meta".into(),
                key: LOG_START_KEY.into(),
                value: serde_json::to_vec(&stop)?,
            });
            self.backend.batch(ops)?;
            at = stop;
        }
        Ok(end - start)
    }

    /// The schema version recorded in this store. See [`SCHEMA_VERSION`].
    pub fn schema_version(&self) -> Result<u32> {
        Ok(self.get_meta::<u32>(SCHEMA_KEY)?.unwrap_or(1))
    }

    /// Rows and bytes per table, and how much of `meta` is the log.
    pub fn stats(&self) -> Result<Vec<TableStats>> {
        self.backend.stats()
    }

    /// Term of the log row at `index`, when this store holds that row.
    pub fn log_term_at(&self, index: u64) -> Result<Option<u64>> {
        if index == 0 {
            return Ok(None);
        }
        Ok(self
            .get_meta::<LogEntry>(&log_key(index))?
            .map(|entry| entry.term))
    }

    /// Apply follower entries. Recording is off for the duration, then restored.
    /// Each entry's index must be one past the one before it. Each entry is
    /// applied and kept as a log row with its original term, so a follower
    /// that later leads can serve it and votes compare real terms. The
    /// entries are committed together: a gap applies none of them and
    /// returns `StoreError::Conflict`.
    ///
    /// A committed page is announced on the event bus: one event per logged
    /// entry and per resource row it changed, in log order, in the shape a
    /// write through a `Collection` sends. A follower applies the leader's
    /// rows straight to the backend, so without this nothing on this node
    /// ever hears that a row appeared or went away — a watcher keeps whatever
    /// it read the first time and is never told to look again.
    ///
    /// An event says what the entry was: a `Created` when the row was not
    /// there before it, an `Updated` when it was, a `Deleted` when the entry
    /// removed the row. Entries are not folded together, so a leader that
    /// created a row and changed it a moment later is heard as two events,
    /// exactly as it was on the node that led.
    pub fn apply_entries(&self, entries: &[LogEntry]) -> Result<()> {
        // Whether a put created a row or changed one is settled here, before
        // anything is written: the page lands as one transaction, so once it
        // commits every row holds what the leader wrote and none of them says
        // what was there first. One read transaction answers for the whole page
        // — every page of a catch-up would otherwise pay for its own.
        let changes = self.plan(entries)?;
        let prev = self.backend.record_log();
        self.set_record_log(false);
        self.backend.set_applying(true);
        // The whole page goes in one commit. A commit per entry costs a disk
        // sync each, the same price the leader paid to write it, so a
        // follower that fell behind a busy leader never caught up.
        let result = (|| {
            let mut have = self.commit_index()?;
            let mut ops: Vec<Op> = Vec::new();
            for entry in entries {
                if entry.index != have + 1 {
                    return Err(StoreError::Conflict(format!(
                        "gap: have {have}, got {}",
                        entry.index
                    )));
                }
                ops.extend(entry.ops.iter().map(op_from_log));
                ops.push(Op::Put {
                    table: "meta".into(),
                    key: log_key(entry.index),
                    value: serde_json::to_vec(entry)?,
                });
                have = entry.index;
            }
            if ops.is_empty() {
                return Ok(());
            }
            ops.push(Op::Put {
                table: "meta".into(),
                key: "cluster_commit".into(),
                value: serde_json::to_vec(&have)?,
            });
            self.backend.batch(ops)
        })();
        self.backend.set_applying(false);
        self.set_record_log(prev);
        result?;
        // The batch is on disk. A page that was refused announced nothing above
        // and announces nothing here: a watcher left holding what it already had
        // is still holding the truth.
        for change in changes {
            let _ = self.events.send(change.into_event());
        }
        Ok(())
    }

    /// What a page of the leader's entries announces: one event per write to a
    /// resource row, in log order, with the action the write was. A put of a row
    /// that was not there before it is a create, of a row that was, an update; a
    /// delete of a row that was there is a delete. A delete of a row that was
    /// already gone changed nothing and tells nobody anything.
    ///
    /// The page is walked in order with a row's state held as it goes, so a row
    /// written twice by one entry is the two writes it was — made, then changed
    /// — and a row a later entry touches is judged by what the entries before it
    /// left, not by the store the page began against.
    ///
    /// A row is asked of the store only when the page cannot answer for it, and
    /// every row of the page out of one consistent read, so a page of a
    /// catch-up is one read rather than one per row. A backend that cannot hand
    /// one out is asked a row at a time through [`Backend::get`].
    fn plan(&self, entries: &[LogEntry]) -> Result<Vec<Change>> {
        let tx = self.backend.whole_store_read();
        let read = match &tx {
            Some(tx) => Read::Tx(tx),
            None => Read::Rows(&self.backend),
        };
        // Whether each row this page writes stands, after the writes of it read
        // so far. A row answered from here needs no read of the store: what an
        // earlier write of this page left is what the next write of it lands on,
        // and the store cannot say that, because the page lands as one
        // transaction and holds only the last word by the time it is readable.
        // A row is named by its table and key together: the same key can be a
        // `names` entry and a `resources` row, and those are different rows.
        let mut stands: Vec<(&str, &str, bool)> = Vec::new();
        let mut out: Vec<Change> = Vec::new();
        for entry in entries {
            for op in &entry.ops {
                let (table, key) = (op.table.as_str(), op.key.as_str());
                let puts = op.value.is_some();
                let seen = stands.iter().position(|(t, k, _)| *t == table && *k == key);
                // What stood before this write.
                let was_there = match seen {
                    Some(at) => stands[at].2,
                    None => match known(&read, op)? {
                        Some(found) => {
                            let stood = found.stood;
                            stands.push((table, key, puts));
                            out.push(found);
                            stood
                        }
                        // Not a row anybody watches: the log's own rows, counters,
                        // name index entries, the audit trail.
                        None => {
                            stands.push((table, key, puts));
                            continue;
                        }
                    },
                };
                if let Some((_, _, state)) =
                    stands.iter_mut().find(|(t, k, _)| *t == table && *k == key)
                {
                    *state = puts;
                }
                // Removed by a write that found nothing there: nothing changed,
                // and nobody is told it did.
                if !was_there && !puts {
                    continue;
                }
                match seen {
                    // The page's first write of the row, announced by the read
                    // above: the store's answer settles what that write was.
                    None => {
                        if let Some(last) = out.last_mut() {
                            last.action = action_for(was_there, puts);
                        }
                    }
                    // A later write of a row this page already wrote. The name
                    // comes from the page's own change for it, since a row just
                    // deleted has nothing left to be read from.
                    Some(_) => {
                        let mut change = match out.iter().rev().find(|c| c.key == key) {
                            Some(c) => c.clone(),
                            None => continue,
                        };
                        change.action = action_for(was_there, puts);
                        if !puts {
                            change.at = None;
                            change.revision = 0;
                        }
                        out.push(change);
                    }
                }
            }
        }
        Ok(out)
    }

    /// Tell every watcher on this node that everything it derived from the
    /// store may be wrong: the rows behind it were replaced wholesale, most
    /// recently by a snapshot from another server, and one change per row
    /// would be a page of history this node has no copy of.
    ///
    /// A per-kind event cannot do it. A watcher waiting for zones sleeps
    /// straight through an event about volumes, and a watcher cannot be told
    /// every kind: modules name their own and the store does not know them.
    /// Every subscriber already has one path that means "you missed
    /// something, start again" — the bus overflowing, [`Lagged`] — so this
    /// fills the bus past its capacity and leaves every receiver on that
    /// path. The fillers are sent, not held: what a watcher acts on is the
    /// gap they open, not their contents.
    ///
    /// [`Lagged`]: tokio::sync::broadcast::error::RecvError::Lagged
    pub fn announce_all_changed(&self) {
        if self.events.receiver_count() == 0 {
            return;
        }
        // The bus holds at most this many events before it starts dropping,
        // and is never grown. One more than the biggest channel in the process
        // reaches every receiver, whoever subscribed late and whoever was
        // already behind.
        let fill = ANNOUNCE_ALL_FILL;
        let filler = Event {
            kind: String::new(),
            // Names no kind: a filler is there to open a gap in the bus, not to
            // be read as a row of something.
            id: Id::new(""),
            project: None,
            action: EventAction::Updated,
            revision: 0,
            at: Utc::now(),
        };
        for _ in 0..fill {
            let _ = self.events.send(filler.clone());
        }
        tracing::debug!(
            filled = fill,
            "store: every watcher told the whole store may have changed"
        );
    }

    /// Copy the store while this process holds it open. Does not open `src`;
    /// the backend writes a consistent copy (see [`Backend::snapshot_to`]).
    /// `dst` must not already exist. A copy that will not open is removed.
    pub fn snapshot_file(&self, src: &Path, dst: &Path) -> Result<()> {
        if dst.exists() {
            return Err(StoreError::Storage(format!(
                "{} already exists",
                dst.display()
            )));
        }
        if let Some(parent) = dst.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(|e| StoreError::Storage(e.to_string()))?;
            }
        }
        if let Err(err) = self.backend.snapshot_to(src, dst) {
            let _ = std::fs::remove_file(dst);
            return Err(err);
        }
        let check = match Database::open(dst) {
            Ok(db) => db,
            Err(e) => {
                let _ = std::fs::remove_file(dst);
                return Err(e.into());
            }
        };
        if let Err(e) = check.begin_read() {
            drop(check);
            let _ = std::fs::remove_file(dst);
            return Err(e.into());
        }
        Ok(())
    }

    fn res_key(kind_: &str, id: &Id) -> String {
        format!("{kind_}/{id}")
    }
    fn name_key(kind_: &str, project: Option<&Id>, name: &str) -> String {
        match project {
            Some(p) => format!("{kind_}/{p}/{name}"),
            None => format!("{kind_}/-/{name}"),
        }
    }

    fn name_entry(kind_: &str, meta: &Meta) -> Op {
        Op::Put {
            table: "names".into(),
            key: Self::name_key(kind_, meta.project.as_ref(), &meta.name),
            value: meta.id.to_string().into_bytes(),
        }
    }

    /// Move one row to another project and take its name index entry with it.
    /// The resource row and both index keys land in one batch, so a name is
    /// never in two projects or in none — a name lookup reads the index and a
    /// list reads the row, and one of them would otherwise still answer for
    /// the project that gave the row away. A name already taken in the project
    /// asked for refuses the move and leaves the row where it was.
    pub fn reproject<K: Kind>(&self, id: &Id, project: Option<&Id>) -> Result<K> {
        let _g = self.write_lock.write();
        let col = self.collection::<K>();
        let cur = col.get(id)?;
        if cur.meta().project.as_ref() == project {
            return Ok(cur);
        }
        let wanted = Store::name_key(K::KIND, project, &cur.meta().name);
        if self.backend.get("names", &wanted)?.is_some() {
            return Err(StoreError::Conflict(format!(
                "{} named '{}' already exists",
                K::KIND,
                cur.meta().name
            )));
        }
        let old = cur.meta().clone();
        let moved_meta = {
            let mut m = old.clone();
            m.project = project.cloned();
            m.touch();
            m
        };
        let mut item = cur;
        *item.meta_mut() = moved_meta.clone();
        self.backend.batch(vec![
            Op::Delete {
                table: "names".into(),
                key: Store::name_key(K::KIND, old.project.as_ref(), &old.name),
            },
            Op::Put {
                table: "resources".into(),
                key: Store::res_key(K::KIND, &moved_meta.id),
                value: serde_json::to_vec(&item)?,
            },
            Store::name_entry(K::KIND, &moved_meta),
        ])?;
        col.emit(&moved_meta, EventAction::Updated);
        Ok(item)
    }
}

/// Typed view over one resource kind.
pub struct Collection<K: Kind> {
    store: Store,
    _k: std::marker::PhantomData<K>,
}

impl<K: Kind> Clone for Collection<K> {
    fn clone(&self) -> Self {
        Self {
            store: self.store.clone(),
            _k: std::marker::PhantomData,
        }
    }
}

impl<K: Kind> Collection<K> {
    pub fn get(&self, id: &Id) -> Result<K> {
        let key = Store::res_key(K::KIND, id);
        match self.store.backend.get("resources", &key)? {
            Some(b) => Ok(serde_json::from_slice(&b)?),
            None => Err(StoreError::NotFound(format!("{} {id} not found", K::KIND))),
        }
    }

    pub fn try_get(&self, id: &Id) -> Result<Option<K>> {
        match self.get(id) {
            Ok(v) => Ok(Some(v)),
            Err(StoreError::NotFound(_)) => Ok(None),
            Err(e) => Err(e),
        }
    }

    pub fn get_by_name(&self, project: Option<&Id>, name: &str) -> Result<K> {
        let nk = Store::name_key(K::KIND, project, name);
        let id_bytes = self
            .store
            .backend
            .get("names", &nk)?
            .ok_or_else(|| StoreError::NotFound(format!("{} '{name}' not found", K::KIND)))?;
        let id_str = String::from_utf8(id_bytes).map_err(|e| StoreError::Storage(e.to_string()))?;
        let id: Id = serde_json::from_value(serde_json::Value::String(id_str))?;
        self.get(&id)
    }

    /// Resolve `name-or-id` the way the CLI does.
    pub fn resolve(&self, project: Option<&Id>, name_or_id: &str) -> Result<K> {
        if let Some(id) = Id::parse(intern(K::KIND), name_or_id) {
            return self.get(&id);
        }
        self.get_by_name(project, name_or_id)
    }

    /// Resolve `name-or-id` for a caller who may reach the whole cloud.
    ///
    /// An id — whole, or the short tail a table prints — is the row's own
    /// address and is answered whole. A name is looked up in `project` and then
    /// in the cloud scope: a row created through a token holding no project is
    /// filed under the bare name, and the caller listing it usually holds one.
    /// A caller of no project therefore resolves the same row its own list
    /// printed, rather than being handed a name it cannot use. What the caller
    /// may then touch is the caller's own rule; this is only where the row is
    /// found.
    ///
    /// The name order is deliberately not reversed. A name is unique inside a
    /// project and separately unique in the cloud scope, so the two can hold
    /// different rows of the same name; the caller's own project wins, exactly
    /// as it does for [`Self::resolve`].
    pub fn resolve_scoped(&self, project: Option<&Id>, name_or_id: &str) -> Result<K> {
        if let Some(id) = self.addressed(name_or_id)? {
            return self.get(&id);
        }
        match self.find_by_name(project, name_or_id)? {
            Some(k) => Ok(k),
            None if project.is_some() => self.get_by_name(None, name_or_id),
            None => Err(StoreError::NotFound(format!(
                "{} '{name_or_id}' not found",
                K::KIND
            ))),
        }
    }

    /// The id of the row this collection holds that `text` is the address of, if
    /// any: a whole id of this kind, or the short form a table prints.
    ///
    /// A whole id is answered straight from the row key — the resources table is
    /// keyed `{kind}/{id}`, so a lookup by a valid id cannot be beaten by a name
    /// that looks the same way. This matters more than it looks: names may
    /// contain hyphens and hex, so a long hyphenated name is indistinguishable
    /// from an id to a parser that only reads text, and a service that generates
    /// a resource name out of another resource's full id — a restored volume is
    /// named after the backup it came from — then produces rows whose names are
    /// id-shaped. Resolving those names before the row's own address lets a
    /// project own the name and quietly answer a different row than the caller
    /// pointed at, or answer `not found` for a row it can see in its own list.
    ///
    /// A whole id that names no row is not an answer: it falls through to the
    /// name lookup, because an id that was deleted is a name nobody has taken.
    fn addressed(&self, text: &str) -> Result<Option<Id>> {
        if let Some(id) = Id::parse(intern(K::KIND), text) {
            if self.try_get(&id)?.is_some() {
                return Ok(Some(id));
            }
        }
        displayed_id(self, K::KIND, text)
    }

    /// [`Self::get_by_name`] as `None` rather than an error.
    fn find_by_name(&self, project: Option<&Id>, name: &str) -> Result<Option<K>> {
        match self.get_by_name(project, name) {
            Ok(k) => Ok(Some(k)),
            Err(StoreError::NotFound(_)) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// All resources of this kind, optionally one project's.
    pub fn list(&self, project: Option<&Id>) -> Result<Vec<K>> {
        let prefix = format!("{}/", K::KIND);
        let mut out = Vec::new();
        for (_, v) in self.store.backend.scan("resources", &prefix)? {
            let item: K = serde_json::from_slice(&v)?;
            if let Some(p) = project {
                if item.meta().project.as_ref() != Some(p) {
                    continue;
                }
            }
            out.push(item);
        }
        Ok(out)
    }

    /// List with a label selector (all pairs must match).
    pub fn list_where(
        &self,
        project: Option<&Id>,
        labels: &BTreeMap<String, String>,
    ) -> Result<Vec<K>> {
        Ok(self
            .list(project)?
            .into_iter()
            .filter(|k| {
                labels
                    .iter()
                    .all(|(lk, lv)| k.meta().labels.get(lk) == Some(lv))
            })
            .collect())
    }

    /// Create. Fails with Conflict if the name is taken in that project.
    pub fn create(&self, mut item: K) -> Result<K> {
        let _g = self.store.write_lock.write();
        let m = item.meta_mut();
        m.revision = 1;
        let meta: Meta = m.clone();
        let nk = Store::name_key(K::KIND, meta.project.as_ref(), &meta.name);
        if self.store.backend.get("names", &nk)?.is_some() {
            return Err(StoreError::Conflict(format!(
                "{} named '{}' already exists",
                K::KIND,
                meta.name
            )));
        }
        let rk = Store::res_key(K::KIND, &meta.id);
        self.store.backend.batch(vec![
            Op::Put {
                table: "resources".into(),
                key: rk,
                value: serde_json::to_vec(&item)?,
            },
            Op::Put {
                table: "names".into(),
                key: nk,
                value: meta.id.to_string().into_bytes(),
            },
        ])?;
        self.emit(&meta, EventAction::Created);
        Ok(item)
    }

    /// Replace. Bumps revision. If `expected_revision` is given, enforces it.
    pub fn update(&self, mut item: K, expected_revision: Option<u64>) -> Result<K> {
        let _g = self.store.write_lock.write();
        let current = self.get(&item.meta().id)?;
        let cur_rev = current.meta().revision;
        if let Some(exp) = expected_revision {
            if exp != cur_rev {
                return Err(StoreError::Revision {
                    expected: exp,
                    found: cur_rev,
                });
            }
        }
        let mut ops = Vec::new();
        // The name index is keyed by project as well as name, so a write that
        // changes either has to move the entry and drop the old key: a rename,
        // and a move to another project, which keeps the name and changes the
        // project. Left in place, the row answers to its former name or its
        // former project forever.
        let renamed = current.meta().name != item.meta().name
            || current.meta().project != item.meta().project;
        if renamed {
            let old = Store::name_key(
                K::KIND,
                current.meta().project.as_ref(),
                &current.meta().name,
            );
            let new = Store::name_key(K::KIND, item.meta().project.as_ref(), &item.meta().name);
            if self.store.backend.get("names", &new)?.is_some() {
                return Err(StoreError::Conflict(format!(
                    "{} named '{}' already exists",
                    K::KIND,
                    item.meta().name
                )));
            }
            ops.push(Op::Delete {
                table: "names".into(),
                key: old,
            });
            ops.push(Op::Put {
                table: "names".into(),
                key: new,
                value: item.meta().id.to_string().into_bytes(),
            });
        }
        {
            let m = item.meta_mut();
            m.revision = cur_rev;
            m.touch();
        }
        let meta = item.meta().clone();
        ops.push(Op::Put {
            table: "resources".into(),
            key: Store::res_key(K::KIND, &meta.id),
            value: serde_json::to_vec(&item)?,
        });
        self.store.backend.batch(ops)?;
        self.emit(&meta, EventAction::Updated);
        Ok(item)
    }

    /// Read-modify-write helper with retry on revision conflict.
    pub fn modify(&self, id: &Id, mut f: impl FnMut(&mut K) -> Result<()>) -> Result<K> {
        for _ in 0..8 {
            let mut cur = self.get(id)?;
            let rev = cur.meta().revision;
            f(&mut cur)?;
            match self.update(cur, Some(rev)) {
                Ok(v) => return Ok(v),
                Err(StoreError::Revision { .. }) => continue,
                Err(e) => return Err(e),
            }
        }
        Err(StoreError::Conflict(format!(
            "{} {id}: too many concurrent writers",
            K::KIND
        )))
    }

    pub fn delete(&self, id: &Id) -> Result<K> {
        let _g = self.store.write_lock.write();
        let cur = self.get(id)?;
        let meta = cur.meta().clone();
        self.store.backend.batch(vec![
            Op::Delete {
                table: "resources".into(),
                key: Store::res_key(K::KIND, id),
            },
            Op::Delete {
                table: "names".into(),
                key: Store::name_key(K::KIND, meta.project.as_ref(), &meta.name),
            },
        ])?;
        self.emit(&meta, EventAction::Deleted);
        Ok(cur)
    }

    pub fn count(&self, project: Option<&Id>) -> Result<usize> {
        Ok(self.list(project)?.len())
    }

    fn emit(&self, meta: &Meta, action: EventAction) {
        let _ = self.store.events.send(Event {
            kind: K::KIND.to_owned(),
            id: meta.id.clone(),
            project: meta.project.clone(),
            action,
            revision: meta.revision,
            at: meta.updated_at,
        });
    }
}

/// Interns `s` into a `&'static str`, for an `Id`'s kind, which the type keeps
/// by reference rather than by copy.
///
/// A kind is one of a handful of `KIND` constants, but a caller that reaches an
/// id through a key or a name does not hold one of those — it holds a slice of
/// a key read off disk. Handing that to `Id::parse` needs a `'static` string.
/// Leaking one per call would grow the heap on every lookup and, when a
/// follower applies a page, on every replicated row. So the set of kinds ever
/// seen is kept once, process-wide, and each is leaked at most once: a repeat
/// returns the string the first call stored, so equal text is ever the same
/// pointer.
fn intern(s: &str) -> &'static str {
    static KINDS: OnceLock<parking_lot::Mutex<HashSet<&'static str>>> = OnceLock::new();
    let seen = KINDS.get_or_init(|| parking_lot::Mutex::new(HashSet::new()));
    let mut kinds = seen.lock();
    if let Some(&held) = kinds.get(s) {
        return held;
    }
    let fresh: &'static str = Box::leak(s.to_owned().into_boxed_str());
    kinds.insert(fresh);
    fresh
}

/// How many hex digits of an id a display keeps, and the shortest run that
/// is read back as an id rather than a name.
const SHORT_ID_HEX: usize = 8;
const SHORT_ID_MIN: usize = 4;

/// The id as a table column prints it: the kind prefix and the last
/// [`SHORT_ID_HEX`] hex digits of the uuid.
pub fn short_id(id: &Id) -> String {
    let hex = id.uuid.simple().to_string();
    format!("{}-{}", id.kind, &hex[hex.len() - SHORT_ID_HEX..])
}

/// Read back what [`short_id`] wrote as the id of `prefix`, or a full id,
/// against the rows of `col`.
///
/// The short form is what a table shows, so it is what an operator copies
/// back. The tail is not unique on its own — see [`find_by_short_id`] — and
/// a run shorter than [`SHORT_ID_MIN`] is taken for what it most likely is:
/// a name, so this answers `None` and the caller looks the name up.
pub fn displayed_id<K: Kind>(col: &Collection<K>, prefix: &str, text: &str) -> Result<Option<Id>> {
    if let Some(id) = Id::parse(intern(prefix), text) {
        return Ok(Some(id));
    }
    // The prefix is what the tool that printed the id calls the kind, which is
    // not always the kind itself: a table's ID column prints a backup as
    // `bak-1a2b3c4d` while the row's kind is `backup`. An id read back has to
    // be tried under the name it was written with as well as under the kind,
    // or every short id from that table is a word the service cannot find.
    for kind in [K::KIND, prefix] {
        if let Some(tail) = short_tail_of(kind, text) {
            return find_by_short_id(col, &tail);
        }
    }
    Ok(None)
}

/// The tail [`short_id`] keeps, when `text` is that form of an id of
/// `prefix`. `None` when `text` is neither a full id of `prefix` nor
/// `prefix-<tail>`.
pub fn short_tail_of(prefix: &str, text: &str) -> Option<String> {
    if Id::parse(intern(prefix), text).is_some() {
        return None;
    }
    text.strip_prefix(prefix)
        .and_then(|r| r.strip_prefix('-'))
        .filter(|t| t.len() >= SHORT_ID_MIN && t.len() < 32)
        .filter(|t| t.chars().all(|c| c.is_ascii_hexdigit()))
        .map(|t| t.to_ascii_lowercase())
}

/// The row of `col` whose id ends in `tail`. A v7 uuid leads with a
/// millisecond clock, so a tail is unique while rows are made apart; a tail
/// two rows do share is refused rather than guessed.
pub fn find_by_short_id<K: Kind>(col: &Collection<K>, tail: &str) -> Result<Option<Id>> {
    let hits: Vec<Id> = col
        .list(None)?
        .into_iter()
        .map(|item| item.meta().id.clone())
        .filter(|id| id.uuid.simple().to_string().ends_with(tail))
        .collect();
    match hits.len() {
        0 => Ok(None),
        1 => Ok(hits.into_iter().next()),
        _ => Err(StoreError::Conflict(format!(
            "short id '{tail}' is ambiguous: {} rows end in it",
            hits.len()
        ))),
    }
}

/// What [`check_file`] read from a store file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreFileInfo {
    /// The schema version the file records; 1 when it has no marker.
    pub schema_version: u32,
    pub resources: u64,
    pub names: u64,
    pub meta: u64,
}

/// Check that `path` is a whole store: redb opens it and its pages pass
/// redb's checksums, the three tables are there and every row of each can
/// be read. Nothing is migrated and no marker is written, but redb may
/// finish an interrupted commit in the file, so give it a copy and not
/// somebody's only backup. A file that is missing, cut short or damaged is
/// an error.
pub fn check_file(path: &Path) -> Result<StoreFileInfo> {
    if !path.is_file() {
        return Err(StoreError::Storage(format!(
            "{} is not a file",
            path.display()
        )));
    }
    // redb asserts on some damage, a file shorter than its header says for
    // one, where it could return an error. Here that is an answer too.
    std::panic::catch_unwind(|| read_whole_file(path)).unwrap_or_else(|_| {
        Err(StoreError::Storage(
            "redb gave up reading the file; it is cut short or damaged".into(),
        ))
    })
}

fn read_whole_file(path: &Path) -> Result<StoreFileInfo> {
    let mut db = Database::open(path)?;
    if !db.check_integrity()? {
        return Err(StoreError::Storage(
            "the file failed redb's integrity check".into(),
        ));
    }
    let tx = db.begin_read()?;
    let mut info = StoreFileInfo {
        schema_version: 1,
        resources: 0,
        names: 0,
        meta: 0,
    };
    for item in tx.open_table(RESOURCES)?.iter()? {
        item?;
        info.resources += 1;
    }
    for item in tx.open_table(NAMES)?.iter()? {
        item?;
        info.names += 1;
    }
    for item in tx.open_table(META)?.iter()? {
        let (key, value) = item?;
        if key.value() == SCHEMA_KEY {
            info.schema_version = serde_json::from_slice(value.value())?;
        }
        info.meta += 1;
    }
    Ok(info)
}

/// True when another process, a running server, holds this store file.
pub fn is_held_open(path: &Path) -> bool {
    matches!(
        Database::open(path),
        Err(redb::DatabaseError::DatabaseAlreadyOpen)
    )
}

/// Copy a redb file and check the copy with [`check_file`].
///
/// The write lock is held for the copy, so the bytes stay at the last commit.
/// `dst` must not already exist. The copy records the schema version even
/// when `src` was last written before stores had the marker, so a restore
/// can tell what it was given.
pub fn backup_file(src: &Path, dst: &Path) -> Result<StoreFileInfo> {
    if dst.exists() {
        return Err(StoreError::Storage(format!(
            "{} already exists",
            dst.display()
        )));
    }
    if let Some(parent) = dst.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|e| StoreError::Storage(e.to_string()))?;
        }
    }
    match Database::open(src) {
        Ok(db) => {
            let txn = db.begin_write()?;
            copy_db(src, dst)?;
            drop(txn);
        }
        Err(e) => {
            let msg = e.to_string();
            if msg.contains("already open") || msg.contains("lock") {
                copy_db(src, dst)?;
            } else {
                return Err(e.into());
            }
        }
    }
    let checked = check_file(dst).and_then(|info| {
        stamp_schema(dst)?;
        Ok(info)
    });
    if checked.is_err() {
        let _ = std::fs::remove_file(dst);
    }
    checked
}

/// Write the version-1 marker into a store file that has none.
fn stamp_schema(path: &Path) -> Result<()> {
    let db = Database::open(path)?;
    let tx = db.begin_write()?;
    {
        let mut meta = tx.open_table(META)?;
        if meta.get(SCHEMA_KEY)?.is_none() {
            meta.insert(SCHEMA_KEY, serde_json::to_vec(&1u32)?.as_slice())?;
        }
    }
    tx.commit()?;
    Ok(())
}

fn copy_db(src: &Path, dst: &Path) -> Result<()> {
    std::fs::copy(src, dst).map_err(|e| StoreError::Storage(e.to_string()))?;
    Ok(())
}

/// Re-export so modules can name the shared kinds without importing types.
pub use pistack_types::kind as kinds;

#[cfg(test)]
mod tests {
    use super::*;
    use pistack_types::{kind, Meta, Resource};

    #[test]
    fn backup_round_trips_a_value() {
        let dir = std::env::temp_dir().join(format!("pistack-backup-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("state.redb");
        let dst = dir.join("copy.redb");
        let (events, _) = broadcast::channel(8);
        let store = Store::open(&src, events).unwrap();
        store.put_meta("cluster_id", &"abc".to_owned()).unwrap();
        drop(store);
        backup_file(&src, &dst).unwrap();
        let (events, _) = broadcast::channel(8);
        let opened = Store::open(&dst, events).unwrap();
        let id: Option<String> = opened.get_meta("cluster_id").unwrap();
        assert_eq!(id.as_deref(), Some("abc"));
        let err = backup_file(&src, &dst).unwrap_err();
        assert!(err.to_string().contains("already exists"));
        let held = Database::open(&src).unwrap();
        let second = dir.join("live.redb");
        backup_file(&src, &second).unwrap();
        drop(held);
        let (events, _) = broadcast::channel(8);
        let opened = Store::open(&second, events).unwrap();
        let id: Option<String> = opened.get_meta("cluster_id").unwrap();
        assert_eq!(id.as_deref(), Some("abc"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn open_plain(path: &Path) -> Result<Store> {
        let (events, _) = broadcast::channel(8);
        Store::open(path, events)
    }

    /// A store as a binary at `target` with `steps` would open it.
    fn open_as(path: &Path, target: u32, steps: &[Migration]) -> Result<Store> {
        let (events, _) = broadcast::channel(8);
        Ok(Store::new(
            Arc::new(RedbBackend::open_at(path, target, steps)?),
            events,
        ))
    }

    /// A store file as binaries before the marker left it.
    fn unmarked_store(path: &Path) {
        let db = Database::create(path).unwrap();
        let tx = db.begin_write().unwrap();
        {
            tx.open_table(RESOURCES).unwrap();
            tx.open_table(NAMES).unwrap();
            let mut meta = tx.open_table(META).unwrap();
            meta.insert("cluster_id", b"\"abc\"".as_slice()).unwrap();
        }
        tx.commit().unwrap();
    }

    fn trace(tx: &redb::WriteTransaction, step: &str) -> Result<()> {
        let mut meta = tx.open_table(META)?;
        let mut seen: Vec<String> = match meta.get("trace")? {
            Some(raw) => serde_json::from_slice(raw.value())?,
            None => Vec::new(),
        };
        seen.push(step.to_owned());
        meta.insert("trace", serde_json::to_vec(&seen)?.as_slice())?;
        Ok(())
    }
    fn to_two(tx: &redb::WriteTransaction) -> Result<()> {
        trace(tx, "two")
    }
    fn to_three(tx: &redb::WriteTransaction) -> Result<()> {
        trace(tx, "three")
    }
    fn fails(tx: &redb::WriteTransaction) -> Result<()> {
        trace(tx, "half done")?;
        Err(StoreError::Storage("step gave up".into()))
    }
    // Listed out of order on purpose: steps run by version, not by position.
    const STEPS: &[Migration] = &[
        Migration {
            to: 3,
            run: to_three,
        },
        Migration { to: 2, run: to_two },
    ];

    #[test]
    fn the_registered_steps_reach_the_schema_version() {
        // Exactly one step to each version above 1.
        let mut steps: Vec<u32> = MIGRATIONS.iter().map(|step| step.to).collect();
        steps.sort_unstable();
        let wanted: Vec<u32> = (1..SCHEMA_VERSION).map(|version| version + 1).collect();
        assert_eq!(steps, wanted);
    }

    #[test]
    fn a_new_store_records_the_schema_version_and_a_reopen_keeps_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.redb");
        let store = open_plain(&path).unwrap();
        assert_eq!(store.schema_version().unwrap(), SCHEMA_VERSION);
        assert_eq!(
            store.get_meta::<u32>(SCHEMA_KEY).unwrap(),
            Some(SCHEMA_VERSION)
        );
        // The marker is this file's own: it never goes into the log.
        store.set_record_log(true);
        store.put_meta(SCHEMA_KEY, &SCHEMA_VERSION).unwrap();
        assert_eq!(store.commit_index().unwrap(), 0);
        drop(store);
        let store = open_plain(&path).unwrap();
        assert_eq!(store.schema_version().unwrap(), SCHEMA_VERSION);
        assert_eq!(Store::in_memory().unwrap().schema_version().unwrap(), 1);
    }

    #[test]
    fn a_store_with_no_marker_is_version_one() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.redb");
        unmarked_store(&path);
        assert_eq!(check_file(&path).unwrap().schema_version, 1);
        let store = open_plain(&path).unwrap();
        assert_eq!(store.get_meta::<u32>(SCHEMA_KEY).unwrap(), Some(1));
        let id: Option<String> = store.get_meta("cluster_id").unwrap();
        assert_eq!(id.as_deref(), Some("abc"));
    }

    #[test]
    fn migration_steps_run_once_in_order_and_are_recorded() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.redb");
        unmarked_store(&path);
        let trace_of =
            |store: &Store| -> Vec<String> { store.get_meta("trace").unwrap().unwrap_or_default() };

        let store = open_as(&path, 3, STEPS).unwrap();
        assert_eq!(store.schema_version().unwrap(), 3);
        assert_eq!(trace_of(&store), ["two", "three"]);
        let id: Option<String> = store.get_meta("cluster_id").unwrap();
        assert_eq!(id.as_deref(), Some("abc"));
        drop(store);

        // A second open at the same version runs nothing.
        let store = open_as(&path, 3, STEPS).unwrap();
        assert_eq!(store.schema_version().unwrap(), 3);
        assert_eq!(trace_of(&store), ["two", "three"]);
        drop(store);

        // A store already at 2 only takes the step to 3.
        let part = dir.path().join("part.redb");
        drop(open_as(&part, 2, STEPS).unwrap());
        let store = open_as(&part, 2, STEPS).unwrap();
        assert_eq!(trace_of(&store), ["two"]);
        drop(store);
        let store = open_as(&part, 3, STEPS).unwrap();
        assert_eq!(store.schema_version().unwrap(), 3);
        assert_eq!(trace_of(&store), ["two", "three"]);
    }

    #[test]
    fn a_failed_step_changes_nothing_and_a_missing_step_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.redb");
        drop(open_as(&path, 2, STEPS).unwrap());
        let broken = &[Migration { to: 3, run: fails }];
        let err = open_as(&path, 3, broken).err().unwrap().to_string();
        assert!(err.contains("from schema version 2 to 3"), "{err}");
        assert!(err.contains("step gave up"), "{err}");
        let err = open_as(&path, 3, &[]).err().unwrap().to_string();
        assert!(err.contains("no step to version 3"), "{err}");
        let store = open_as(&path, 2, STEPS).unwrap();
        assert_eq!(store.schema_version().unwrap(), 2);
        let seen: Vec<String> = store.get_meta("trace").unwrap().unwrap();
        assert_eq!(seen, ["two"]);
    }

    #[test]
    fn an_older_binary_refuses_a_newer_store_and_leaves_it_alone() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.redb");
        let store = open_as(&path, 3, STEPS).unwrap();
        store.put_meta("cluster_id", &"abc".to_owned()).unwrap();
        drop(store);
        let before = std::fs::read(&path).unwrap();

        let err = open_as(&path, 2, STEPS).err().unwrap();
        assert!(matches!(
            err,
            StoreError::SchemaTooNew {
                found: 3,
                supported: 2
            }
        ));
        let text = err.to_string();
        assert!(text.contains("schema version 3"), "{text}");
        assert!(text.contains("reads up to 2"), "{text}");
        // This binary is older than that store too.
        assert!(matches!(
            open_plain(&path).err().unwrap(),
            StoreError::SchemaTooNew { found: 3, .. }
        ));
        assert_eq!(std::fs::read(&path).unwrap(), before);

        let store = open_as(&path, 3, STEPS).unwrap();
        let id: Option<String> = store.get_meta("cluster_id").unwrap();
        assert_eq!(id.as_deref(), Some("abc"));
    }

    #[test]
    fn a_backup_and_a_snapshot_carry_the_schema_version() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("state.redb");
        let store = open_as(&src, 3, STEPS).unwrap();
        let snap = dir.path().join("snap.redb");
        store.snapshot_file(&src, &snap).unwrap();
        drop(store);
        let copy = dir.path().join("copy.redb");
        assert_eq!(backup_file(&src, &copy).unwrap().schema_version, 3);
        assert_eq!(check_file(&copy).unwrap().schema_version, 3);
        assert_eq!(check_file(&snap).unwrap().schema_version, 3);

        // A store from before the marker is backed up as version 1, in writing.
        let old = dir.path().join("old.redb");
        unmarked_store(&old);
        let copy = dir.path().join("old-copy.redb");
        assert_eq!(backup_file(&old, &copy).unwrap().schema_version, 1);
        let db = Database::open(&copy).unwrap();
        let tx = db.begin_read().unwrap();
        let meta = tx.open_table(META).unwrap();
        assert_eq!(meta.get(SCHEMA_KEY).unwrap().unwrap().value(), b"1");
    }

    #[test]
    fn check_file_refuses_a_file_that_is_cut_short_damaged_or_not_a_store() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("state.redb");
        let store = open_plain(&src).unwrap();
        for i in 0..40 {
            store
                .put_meta(&format!("row/{i:04}"), &"x".repeat(9000))
                .unwrap();
        }
        drop(store);
        let good = std::fs::read(&src).unwrap();
        let info = check_file(&src).unwrap();
        assert_eq!(info.schema_version, SCHEMA_VERSION);
        assert_eq!(info.meta, 41);

        for keep in [good.len() / 2, good.len() - 4096, 100, 0] {
            let cut = dir.path().join(format!("cut-{keep}.redb"));
            std::fs::write(&cut, &good[..keep]).unwrap();
            assert!(check_file(&cut).is_err(), "cut to {keep} bytes");
        }

        // Damage a stored value, not free space: the last row written.
        let mut bad = good.clone();
        let needle = b"row/0039";
        let at = (0..bad.len() - needle.len())
            .rev()
            .find(|at| &bad[*at..*at + needle.len()] == needle)
            .unwrap();
        for byte in &mut bad[at + 64..at + 2048] {
            *byte ^= 0x5a;
        }
        let damaged = dir.path().join("damaged.redb");
        std::fs::write(&damaged, &bad).unwrap();
        assert!(check_file(&damaged).is_err(), "a damaged row");

        let other = dir.path().join("notes.txt");
        std::fs::write(&other, b"not a store").unwrap();
        assert!(check_file(&other).is_err());
        assert!(check_file(&dir.path().join("missing.redb")).is_err());

        // A redb file that is not a pistack store.
        let foreign = dir.path().join("foreign.redb");
        drop(Database::create(&foreign).unwrap());
        assert!(check_file(&foreign).is_err());
    }
    use serde::Deserialize;

    #[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
    struct WidgetSpec {
        size: u32,
    }
    #[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
    struct WidgetStatus {
        ready: bool,
    }
    type Widget = Resource<WidgetSpec, WidgetStatus>;
    impl pistack_types::Spec for WidgetSpec {
        const KIND: &'static str = "widget";
    }

    fn widget(name: &str, project: Option<Id>) -> Widget {
        Widget {
            meta: Meta::new("widget", name, project),
            spec: WidgetSpec { size: 1 },
            status: WidgetStatus::default(),
        }
    }

    #[test]
    fn put_and_delete_meta_is_one_transaction() {
        let store = Store::in_memory().unwrap();
        store.put_meta("old", &"a").unwrap();
        store.set_record_log(true);
        store.put_and_delete_meta("new", &"b", "old").unwrap();
        assert_eq!(
            store.get_meta::<String>("new").unwrap().as_deref(),
            Some("b")
        );
        assert!(store.get_meta::<String>("old").unwrap().is_none());
        // Replicated as one log entry carrying both changes.
        let log = store.read_log(0, 10).unwrap();
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].ops.len(), 2);
        assert!(store.delete_meta("new").unwrap());
        assert!(!store.delete_meta("new").unwrap());
    }

    #[test]
    fn a_store_file_opens_once() {
        let dir = std::env::temp_dir().join(format!("pistack-open-once-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("state.redb");
        let (events, _) = broadcast::channel(8);
        let first = Store::open(&path, events.clone()).unwrap();
        // What keeps `pistack keep …` off a store a running server holds.
        assert!(Store::open(&path, events.clone()).is_err());
        drop(first);
        assert!(Store::open(&path, events).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn crud_and_names() {
        let store = Store::in_memory().unwrap();
        let col = store.collection::<Widget>();
        let proj = Id::new(kind::PROJECT);
        let w = col.create(widget("a", Some(proj.clone()))).unwrap();
        assert_eq!(col.get(&w.meta.id).unwrap().spec.size, 1);
        assert_eq!(
            col.get_by_name(Some(&proj), "a").unwrap().meta.id,
            w.meta.id
        );
        assert!(matches!(
            col.create(widget("a", Some(proj.clone()))),
            Err(StoreError::Conflict(_))
        ));
        // same name in another project is fine
        let other = Id::new(kind::PROJECT);
        col.create(widget("a", Some(other.clone()))).unwrap();
        assert_eq!(col.list(Some(&proj)).unwrap().len(), 1);
        assert_eq!(col.list(None).unwrap().len(), 2);

        let updated = col
            .modify(&w.meta.id, |x| {
                x.spec.size = 5;
                Ok(())
            })
            .unwrap();
        assert_eq!(updated.meta.revision, 2);
        assert!(matches!(
            col.update(w.clone(), Some(1)),
            Err(StoreError::Revision { .. })
        ));

        col.delete(&w.meta.id).unwrap();
        assert!(matches!(col.get(&w.meta.id), Err(StoreError::NotFound(_))));
        assert!(matches!(
            col.get_by_name(Some(&proj), "a"),
            Err(StoreError::NotFound(_))
        ));
    }

    #[test]
    fn a_row_moved_to_another_project_answers_to_its_name_there() {
        let store = Store::in_memory().unwrap();
        let col = store.collection::<Widget>();
        let from = Id::new(kind::PROJECT);
        let to = Id::new(kind::PROJECT);
        let w = col.create(widget("a", Some(from.clone()))).unwrap();

        let moved = store.reproject::<Widget>(&w.meta.id, Some(&to)).unwrap();
        assert_eq!(moved.meta.project.as_ref(), Some(&to));
        assert_eq!(moved.meta.name, "a");
        assert_eq!(moved.meta.revision, w.meta.revision + 1);
        assert_eq!(col.get_by_name(Some(&to), "a").unwrap().meta.id, w.meta.id);
        assert!(matches!(
            col.get_by_name(Some(&from), "a"),
            Err(StoreError::NotFound(_))
        ));
        assert!(col.list(Some(&from)).unwrap().is_empty());
        assert_eq!(col.list(Some(&to)).unwrap().len(), 1);
        // The name sits under one project, not both.
        assert_eq!(store.raw_scan("names", "widget/").unwrap().len(), 1);

        // A move that comes back, and a move nowhere at all, both hold.
        let back = store.reproject::<Widget>(&w.meta.id, Some(&from)).unwrap();
        assert_eq!(back.meta.project.as_ref(), Some(&from));
        let same = store.reproject::<Widget>(&w.meta.id, Some(&from)).unwrap();
        assert_eq!(same.meta.revision, back.meta.revision);

        // A name already taken in the project asked for refuses the move,
        // and the row stays where it was.
        col.create(widget("a", Some(to.clone()))).unwrap();
        assert!(matches!(
            store.reproject::<Widget>(&w.meta.id, Some(&to)),
            Err(StoreError::Conflict(_))
        ));
        assert_eq!(col.get(&w.meta.id).unwrap().meta.project, Some(from));
    }

    #[test]
    fn events_fire() {
        let store = Store::in_memory().unwrap();
        let mut rx = store.events().subscribe();
        let col = store.collection::<Widget>();
        let w = col.create(widget("x", None)).unwrap();
        let ev = rx.try_recv().unwrap();
        assert_eq!(ev.action, EventAction::Created);
        assert_eq!(ev.id, w.meta.id);
    }

    #[test]
    fn persists_to_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.redb");
        let (tx, _) = broadcast::channel(8);
        {
            let s = Store::open(&path, tx.clone()).unwrap();
            s.collection::<Widget>()
                .create(widget("disk", None))
                .unwrap();
            s.put_meta("cluster_id", &"abc").unwrap();
        }
        let s = Store::open(&path, tx).unwrap();
        assert_eq!(s.collection::<Widget>().list(None).unwrap().len(), 1);
        assert_eq!(s.get_meta::<String>("cluster_id").unwrap().unwrap(), "abc");
    }

    #[test]
    fn record_off_writes_no_log() {
        let store = Store::in_memory().unwrap();
        store.put_meta("cluster_id", &"abc").unwrap();
        assert_eq!(store.commit_index().unwrap(), 0);
        assert!(store.read_log(0, 10).unwrap().is_empty());
    }

    #[test]
    fn record_on_appends_one_entry() {
        let store = Store::in_memory().unwrap();
        store.set_log_term(3);
        store.set_record_log(true);
        store.put_meta("cluster_id", &"abc").unwrap();
        assert_eq!(store.commit_index().unwrap(), 1);
        let log = store.read_log(0, 10).unwrap();
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].index, 1);
        assert_eq!(log[0].term, 3);
        assert!(log[0].ops.iter().any(|op| op.key == "cluster_id"));
        assert!(store.read_log(0, 0).unwrap().is_empty());
        store.put_meta("members", &"[]").unwrap();
        assert_eq!(store.commit_index().unwrap(), 2);
        let second = store.read_log(1, 10).unwrap();
        assert_eq!(second.len(), 1);
        assert_eq!(second[0].index, 2);
        assert!(second[0].ops.iter().any(|op| op.key == "members"));
    }

    #[test]
    fn a_pair_of_meta_keys_is_one_log_entry() {
        let store = Store::in_memory().unwrap();
        store.set_record_log(true);
        store
            .put_meta_pair(("members", &"[]"), ("cluster_removed", &vec!["n2"]))
            .unwrap();
        assert_eq!(store.commit_index().unwrap(), 1);
        let log = store.read_log(0, 10).unwrap();
        assert_eq!(log.len(), 1);
        let keys: Vec<&str> = log[0].ops.iter().map(|op| op.key.as_str()).collect();
        assert_eq!(keys, ["members", "cluster_removed"]);
        let copy = Store::in_memory().unwrap();
        copy.apply_entries(&log).unwrap();
        assert_eq!(
            copy.get_meta::<Vec<String>>("cluster_removed").unwrap(),
            Some(vec!["n2".to_owned()])
        );
        // A server that stood down writes neither half.
        store.set_write_fence(true);
        let err = store
            .put_meta_pair(("members", &"[1]"), ("cluster_removed", &vec!["n3"]))
            .unwrap_err();
        assert!(err.is_not_leader());
        assert_eq!(
            store.get_meta::<String>("members").unwrap().as_deref(),
            Some("[]")
        );
    }

    #[test]
    fn bookkeeping_is_not_a_log_entry() {
        let store = Store::in_memory().unwrap();
        store.set_record_log(true);
        store.put_meta("node_id", &"n1").unwrap();
        store.put_meta("cluster_commit", &7u64).unwrap();
        assert_eq!(store.commit_index().unwrap(), 7);
        assert!(store.read_log(0, 10).unwrap().is_empty());
        assert!(store.raw_scan("meta", "cluster_log/").unwrap().is_empty());
    }

    #[test]
    fn apply_copies_onto_a_quiet_store() {
        let a = Store::in_memory().unwrap();
        a.set_record_log(true);
        a.put_meta("cluster_id", &"abc").unwrap();
        let log = a.read_log(0, 10).unwrap();
        let b = Store::in_memory().unwrap();
        b.apply_entries(&log).unwrap();
        assert_eq!(
            b.get_meta::<String>("cluster_id").unwrap().as_deref(),
            Some("abc")
        );
        assert_eq!(b.read_log(0, 10).unwrap(), log);
        assert_eq!(b.log_term_at(1).unwrap(), Some(log[0].term));
        assert_eq!(b.log_term_at(2).unwrap(), None);
        assert_eq!(b.commit_index().unwrap(), 1);
    }

    #[test]
    fn a_page_is_applied_in_order_and_all_or_nothing() {
        let put = |index: u64, value: &str| LogEntry {
            index,
            term: 2,
            ops: vec![LogOp {
                table: "meta".into(),
                key: "cluster_id".into(),
                value: Some(serde_json::to_vec(value).unwrap()),
            }],
        };
        let store = Store::in_memory().unwrap();
        let page = [put(1, "first"), put(2, "second"), put(3, "third")];
        store.apply_entries(&page).unwrap();
        assert_eq!(
            store.get_meta::<String>("cluster_id").unwrap().as_deref(),
            Some("third"),
            "a later entry in the page wins"
        );
        assert_eq!(store.commit_index().unwrap(), 3);
        assert_eq!(store.read_log(0, 10).unwrap(), page);

        // Entry 6 is missing: 4 and 5 are not applied either.
        let holed = [put(4, "fourth"), put(5, "fifth"), put(7, "seventh")];
        let err = store.apply_entries(&holed).unwrap_err();
        assert!(err.to_string().contains("gap"), "{err}");
        assert_eq!(store.commit_index().unwrap(), 3);
        assert_eq!(
            store.get_meta::<String>("cluster_id").unwrap().as_deref(),
            Some("third")
        );
        assert_eq!(store.read_log(3, 10).unwrap(), []);

        store.apply_entries(&[]).unwrap();
        assert_eq!(store.commit_index().unwrap(), 3);
    }

    #[test]
    fn apply_rejects_a_gap() {
        let store = Store::in_memory().unwrap();
        let entry = LogEntry {
            index: 2,
            term: 1,
            ops: vec![LogOp {
                table: "meta".into(),
                key: "cluster_id".into(),
                value: Some(serde_json::to_vec("abc").unwrap()),
            }],
        };
        let err = store
            .apply_entries(std::slice::from_ref(&entry))
            .unwrap_err();
        assert!(err.to_string().contains("gap"), "{err}");
        assert!(store.get_meta::<String>("cluster_id").unwrap().is_none());
    }

    /// A leader's write, as the follower receives it: one entry per logged
    /// batch, in log order.
    fn page(leader: &Store) -> Vec<LogEntry> {
        leader.read_log(0, 64).unwrap()
    }

    fn drained(rx: &mut broadcast::Receiver<Event>) -> Vec<(String, Id, EventAction)> {
        let mut out = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            out.push((ev.kind, ev.id, ev.action));
        }
        out
    }

    #[test]
    fn a_follower_announces_the_rows_it_applies() {
        let leader = Store::in_memory().unwrap();
        leader.set_record_log(true);
        let col = leader.collection::<Widget>();
        let w = col.create(widget("a", None)).unwrap();
        let updated = col.update(w.clone(), None).unwrap();
        col.delete(&w.meta.id).unwrap();

        let follower = Store::in_memory().unwrap();
        let mut rx = follower.events().subscribe();
        follower.apply_entries(&page(&leader)).unwrap();

        let got = drained(&mut rx);
        assert_eq!(
            got,
            vec![
                ("widget".to_owned(), w.meta.id.clone(), EventAction::Created),
                ("widget".to_owned(), w.meta.id.clone(), EventAction::Updated),
                ("widget".to_owned(), w.meta.id, EventAction::Deleted),
            ],
            "one event per logged entry, in the order the leader made them"
        );
        assert_eq!(follower.commit_index().unwrap(), 3);
        assert_eq!(updated.meta.revision, 2);
    }

    #[test]
    fn an_applied_row_carries_the_shape_a_local_write_sends() {
        let leader = Store::in_memory().unwrap();
        leader.set_record_log(true);
        let project = Id::new(kind::PROJECT);
        let w = leader
            .collection::<Widget>()
            .create(widget("a", Some(project.clone())))
            .unwrap();

        let follower = Store::in_memory().unwrap();
        let mut rx = follower.events().subscribe();
        follower.apply_entries(&page(&leader)).unwrap();
        let ev = rx.try_recv().unwrap();
        assert_eq!(ev.kind, "widget");
        assert_eq!(ev.id, w.meta.id);
        assert_eq!(ev.project.as_ref(), Some(&project));
        assert_eq!(ev.action, EventAction::Created);
        assert_eq!(ev.revision, w.meta.revision);
        // A row applied just now, stamped with the row's own time.
        assert!(ev.at >= w.meta.updated_at);
    }

    #[test]
    fn bookkeeping_rows_are_not_announced() {
        let leader = Store::in_memory().unwrap();
        leader.set_record_log(true);
        leader.put_meta("cluster_id", &"abc").unwrap();
        leader
            .collection::<gate_kind::Audit>()
            .create(gate_kind::audit_row())
            .unwrap();

        let follower = Store::in_memory().unwrap();
        let mut rx = follower.events().subscribe();
        follower.apply_entries(&page(&leader)).unwrap();
        assert!(
            drained(&mut rx).is_empty(),
            "the log's own rows and the audit trail are not a change anybody watches"
        );
        assert_eq!(follower.commit_index().unwrap(), 2);
    }

    /// A name index entry is written beside the resource row it points at, and
    /// its key has a name where an id would be. Moving one — a rename, which
    /// deletes the old key and puts a new one — is nothing a watcher watches:
    /// the row it names is announced by its own write, and nothing in a
    /// `names/...` key is a valid id.
    #[test]
    fn a_moved_name_index_sends_nothing_of_its_own() {
        let leader = Store::in_memory().unwrap();
        leader.set_record_log(true);
        let col = leader.collection::<Widget>();
        let w = col.create(widget("first", None)).unwrap();
        let mut renamed = w.clone();
        renamed.meta_mut().name = "second".to_owned();
        col.update(renamed, None).unwrap();

        let entry = LogEntry {
            index: 1,
            term: 1,
            ops: leader
                .read_log(0, 64)
                .unwrap()
                .iter()
                .flat_map(|e| e.ops.iter())
                .filter(|op| op.table == "names")
                .cloned()
                .collect(),
        };
        assert_eq!(
            entry.ops.len(),
            3,
            "the name added at creation, then the old one removed and the new one added"
        );
        let follower = Store::in_memory().unwrap();
        let mut rx = follower.events().subscribe();
        follower
            .apply_entries(std::slice::from_ref(&entry))
            .unwrap();
        assert!(
            drained(&mut rx).is_empty(),
            "a name entry is not a resource row: nothing names itself with it"
        );
    }

    #[test]
    fn a_rejected_page_announces_nothing() {
        let leader = Store::in_memory().unwrap();
        leader.set_record_log(true);
        let w = leader
            .collection::<Widget>()
            .create(widget("a", None))
            .unwrap();

        let follower = Store::in_memory().unwrap();
        let mut rx = follower.events().subscribe();
        // Index 2 with nothing at 1: the page is refused and no row lands.
        let mut holed = page(&leader);
        holed[0].index = 2;
        let err = follower.apply_entries(&holed).unwrap_err();
        assert!(err.to_string().contains("gap"), "{err}");
        assert!(drained(&mut rx).is_empty());
        assert!(follower
            .collection::<Widget>()
            .try_get(&w.meta.id)
            .unwrap()
            .is_none());
    }

    /// One entry that writes the same row twice — a leader folding two of its
    /// own writes into one logged batch — is heard as the two writes it was:
    /// the row made, then changed. The second write is not told as a second
    /// creation, because the entry's own first write is what it landed on.
    #[test]
    fn an_entry_that_writes_a_row_twice_is_a_create() {
        let leader = Store::in_memory().unwrap();
        let col = leader.collection::<Widget>();
        let w = col.create(widget("a", None)).unwrap();
        // One entry, one row, written twice: what was there before the page
        // began is what says whether the follower saw a new row or an old one.
        let entry = LogEntry {
            index: 1,
            term: 1,
            ops: vec![
                LogOp {
                    table: "resources".into(),
                    key: format!("widget/{}", w.meta.id),
                    value: Some(serde_json::to_vec(&w).unwrap()),
                },
                LogOp {
                    table: "resources".into(),
                    key: format!("widget/{}", w.meta.id),
                    value: Some(serde_json::to_vec(&w).unwrap()),
                },
            ],
        };
        let follower = Store::in_memory().unwrap();
        let mut rx = follower.events().subscribe();
        follower
            .apply_entries(std::slice::from_ref(&entry))
            .unwrap();
        assert_eq!(
            drained(&mut rx),
            vec![
                ("widget".to_owned(), w.meta.id.clone(), EventAction::Created),
                ("widget".to_owned(), w.meta.id, EventAction::Updated),
            ],
            "the row was made by the first write, and changed by the second"
        );
    }

    /// The same entry, one step further: a row the entry rewrote after it
    /// existed is an update, and a row the entry then removed is a delete.
    #[test]
    fn an_entry_writes_then_removes_a_row_in_order() {
        let leader = Store::in_memory().unwrap();
        let col = leader.collection::<Widget>();
        let w = col.create(widget("a", None)).unwrap();
        let entry = LogEntry {
            index: 1,
            term: 1,
            ops: vec![
                LogOp {
                    table: "resources".into(),
                    key: format!("widget/{}", w.meta.id),
                    value: Some(serde_json::to_vec(&w).unwrap()),
                },
                LogOp {
                    table: "resources".into(),
                    key: format!("widget/{}", w.meta.id),
                    value: None,
                },
            ],
        };
        let follower = Store::in_memory().unwrap();
        let mut rx = follower.events().subscribe();
        follower
            .apply_entries(std::slice::from_ref(&entry))
            .unwrap();
        assert_eq!(
            drained(&mut rx),
            vec![
                ("widget".to_owned(), w.meta.id.clone(), EventAction::Created),
                ("widget".to_owned(), w.meta.id.clone(), EventAction::Deleted),
            ]
        );
        assert!(follower
            .collection::<Widget>()
            .try_get(&w.meta.id)
            .unwrap()
            .is_none());
    }

    #[test]
    fn a_replaced_store_is_announced_as_everything_having_changed() {
        let store = Store::in_memory().unwrap();
        let mut zones = store.events().subscribe();
        let mut other = store.events().subscribe();
        // A subscriber that never reads is behind, and is still told.
        store.announce_all_changed();
        for rx in [&mut zones, &mut other] {
            assert!(
                matches!(
                    rx.try_recv(),
                    Err(broadcast::error::TryRecvError::Lagged(_))
                ),
                "every receiver lands on the path that means start again"
            );
        }
    }

    /// The fill is a fixed count, so it has to be enough for the widest
    /// channel any subscriber can hold — not merely wider than the one this
    /// test happens to use. A receiver that keeps up and drains every event
    /// would otherwise sit out the announcement and go on serving a store that
    /// is no longer there, which is the whole fault this is meant to close.
    #[test]
    fn the_fill_overflows_the_widest_channel_a_subscriber_can_have() {
        // The server's own bus, as `pistack serve` builds it.
        let (events, _) = broadcast::channel::<Event>(4096);
        let store = Store::new(Arc::new(RedbBackend::in_memory().unwrap()), events);
        let mut quick = store.events().subscribe();
        let mut slow = store.events().subscribe();
        store.announce_all_changed();
        for rx in [&mut quick, &mut slow] {
            assert!(
                matches!(
                    rx.try_recv(),
                    Err(broadcast::error::TryRecvError::Lagged(_))
                ),
                "a receiver on the widest channel is told to start again"
            );
        }
    }

    /// The filler names nothing, so a watcher that reads what the bus kept
    /// rather than reacting to the gap cannot act on a row that does not exist.
    #[test]
    fn the_filler_names_no_row_a_watcher_could_act_on() {
        let (events, _) = broadcast::channel::<Event>(2);
        let store = Store::new(Arc::new(RedbBackend::in_memory().unwrap()), events);
        let mut rx = store.events().subscribe();
        store.announce_all_changed();
        // Most of the fill is already gone, which is the point: what a receiver
        // gets is the gap they open plus the last few fillers.
        assert!(matches!(
            rx.try_recv(),
            Err(broadcast::error::TryRecvError::Lagged(_))
        ));
        let mut kinds = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            kinds.push(ev.kind);
        }
        assert!(!kinds.is_empty(), "the fill does reach a receiver");
        assert!(
            kinds.iter().all(|k| k.is_empty()),
            "no filler claims a kind a watcher would act on"
        );
    }

    mod gate_kind {
        use super::*;

        /// The gateway's audit row, close enough for the store to apply: the
        /// store knows this kind only as the one it keeps quiet about.
        #[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
        pub struct Audit {
            #[serde(flatten)]
            pub meta: Meta,
        }
        impl Kind for Audit {
            const KIND: &'static str = "audit";
            fn meta(&self) -> &Meta {
                &self.meta
            }
            fn meta_mut(&mut self) -> &mut Meta {
                &mut self.meta
            }
        }
        pub fn audit_row() -> Audit {
            Audit {
                meta: Meta::new("audit", "audit", None),
            }
        }
    }

    #[test]
    fn snapshot_file_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("state.redb");
        let dst = dir.path().join("snap.redb");
        let (events, _) = broadcast::channel(8);
        let store = Store::open(&src, events).unwrap();
        store.put_meta("cluster_id", &"abc").unwrap();
        store.snapshot_file(&src, &dst).unwrap();
        let (events, _) = broadcast::channel(8);
        let opened = Store::open(&dst, events).unwrap();
        assert_eq!(
            opened.get_meta::<String>("cluster_id").unwrap().as_deref(),
            Some("abc")
        );
        let err = store.snapshot_file(&src, &dst).unwrap_err();
        assert!(err.to_string().contains("already exists"), "{err}");
    }

    #[test]
    fn compaction_trims_below_the_commit_row_and_reads_start_after_it() {
        let store = Store::in_memory().unwrap();
        store.set_log_term(2);
        store.set_record_log(true);
        for n in 0..100u64 {
            store.put_meta(&format!("k{n}"), &n).unwrap();
        }
        assert_eq!(store.commit_index().unwrap(), 100);
        assert_eq!(store.log_start().unwrap(), 1);
        // Never past the commit row, and at most `max_rows` per pass.
        assert_eq!(store.compact_log(1_000, 30).unwrap(), 30);
        assert_eq!(store.log_start().unwrap(), 31);
        assert_eq!(store.compact_log(1_000, 1_000).unwrap(), 69);
        assert_eq!(store.log_start().unwrap(), 100);
        assert_eq!(store.log_term_at(100).unwrap(), Some(2));
        assert_eq!(store.log_term_at(99).unwrap(), None);
        assert_eq!(store.compact_log(1_000, 1_000).unwrap(), 0);
        let rows = store.read_log(0, 10).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].index, 100);
        // Trimming is not a log entry, and the state stays.
        assert_eq!(store.commit_index().unwrap(), 100);
        assert_eq!(store.get_meta::<u64>("k0").unwrap(), Some(0));
        // The fence does not refuse it either.
        store.put_meta("k100", &100u64).unwrap();
        store.set_write_fence(true);
        store.set_record_log(false);
        assert_eq!(store.compact_log(1_000, 1_000).unwrap(), 1);
        assert_eq!(store.read_log(0, 10).unwrap()[0].index, 101);
    }

    #[test]
    fn read_log_reads_a_range() {
        let store = Store::in_memory().unwrap();
        store.set_record_log(true);
        for n in 0..10u64 {
            store.put_meta(&format!("k{n}"), &n).unwrap();
        }
        let page = store.read_log(3, 4).unwrap();
        let indexes: Vec<u64> = page.iter().map(|entry| entry.index).collect();
        assert_eq!(indexes, vec![4, 5, 6, 7]);
        assert!(store.read_log(10, 4).unwrap().is_empty());
        assert!(store.read_log(u64::MAX, 4).unwrap().is_empty());
    }

    #[test]
    fn a_snapshot_is_a_fresh_compact_copy_with_every_row_and_stats_count_the_log() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("state.redb");
        let (events, _) = broadcast::channel(8);
        let store = Store::open(&src, events).unwrap();
        store.set_record_log(true);
        let col = store.collection::<Widget>();
        for n in 0..50 {
            col.create(widget(&format!("w{n}"), None)).unwrap();
        }
        let blob = "y".repeat(4096);
        for n in 0..500 {
            store.put_meta(&format!("blob/{n}"), &blob).unwrap();
        }
        for n in 0..500 {
            store.delete_meta(&format!("blob/{n}")).unwrap();
        }
        store
            .compact_log(store.commit_index().unwrap(), u64::MAX)
            .unwrap();
        let dst = dir.path().join("snap.redb");
        store.snapshot_file(&src, &dst).unwrap();
        let (events, _) = broadcast::channel(8);
        let copy = Store::open(&dst, events).unwrap();
        assert_eq!(copy.collection::<Widget>().list(None).unwrap().len(), 50);
        assert!(copy.collection::<Widget>().get_by_name(None, "w7").is_ok());
        assert_eq!(copy.commit_index().unwrap(), store.commit_index().unwrap());
        assert_eq!(copy.log_start().unwrap(), store.log_start().unwrap());
        assert!(
            std::fs::metadata(&dst).unwrap().len() < std::fs::metadata(&src).unwrap().len(),
            "the copy kept free pages"
        );
        let stats = store.stats().unwrap();
        let log = stats
            .iter()
            .find(|t| t.table == "meta:cluster_log")
            .unwrap();
        assert_eq!(log.rows, 1);
        let res = stats.iter().find(|t| t.table == "resources").unwrap();
        assert_eq!(res.rows, 50);
    }

    /// A write that reaches the store after stand-down is refused; the key is
    /// unchanged. Follower apply and membership bookkeeping still succeed.
    #[test]
    fn write_fence_refuses_ordinary_writes_after_stand_down() {
        let store = Store::in_memory().unwrap();
        store.set_log_term(3);
        store.set_record_log(true);
        store.put_meta("kept", &"yes".to_string()).unwrap();
        assert_eq!(store.commit_index().unwrap(), 1);

        // stand_down order: fence on, then logging off.
        store.set_write_fence(true);
        store.set_record_log(false);

        let err = store.put_meta("late", &"no".to_string()).unwrap_err();
        assert!(matches!(err, StoreError::NotLeader), "{err}");
        assert!(store.get_meta::<String>("late").unwrap().is_none());
        assert_eq!(
            store.get_meta::<String>("kept").unwrap().as_deref(),
            Some("yes")
        );
        assert_eq!(store.commit_index().unwrap(), 1);

        // Follow-loop membership writes are allowed and stay out of the log.
        store
            .put_meta("cluster_leader_id", &"other".to_string())
            .unwrap();
        store.put_meta("members", &"[]".to_string()).unwrap();
        store.put_meta("cluster_term", &4u64).unwrap();
        assert_eq!(store.commit_index().unwrap(), 1);

        // Follower apply still copies the leader's log at the next index.
        let entry = LogEntry {
            index: 2,
            term: 4,
            ops: vec![LogOp {
                table: "meta".into(),
                key: "from_leader".into(),
                value: Some(serde_json::to_vec("ok").unwrap()),
            }],
        };
        store.apply_entries(std::slice::from_ref(&entry)).unwrap();
        assert_eq!(
            store.get_meta::<String>("from_leader").unwrap().as_deref(),
            Some("ok")
        );
        assert_eq!(store.commit_index().unwrap(), 2);
    }

    /// A write already inside `batch` when the fence rises is refused: the
    /// wrapper blocks until stand-down finishes, then the real batch sees the
    /// fence and leaves the store unchanged.
    #[test]
    fn write_fence_refuses_a_write_that_commits_after_stand_down() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::mpsc;
        use std::sync::{Arc, Mutex};
        use std::thread;
        use std::time::Duration;

        struct HoldBatch {
            inner: RedbBackend,
            hold: AtomicBool,
            entered: Mutex<Option<mpsc::Sender<()>>>,
            release: Mutex<Option<mpsc::Receiver<()>>>,
        }

        impl Backend for HoldBatch {
            fn get(&self, table: &str, key: &str) -> Result<Option<Vec<u8>>> {
                self.inner.get(table, key)
            }
            fn put(&self, table: &str, key: &str, value: &[u8]) -> Result<()> {
                self.batch(vec![Op::Put {
                    table: table.into(),
                    key: key.into(),
                    value: value.to_vec(),
                }])
            }
            fn delete(&self, table: &str, key: &str) -> Result<bool> {
                let existed = self.get(table, key)?.is_some();
                self.batch(vec![Op::Delete {
                    table: table.into(),
                    key: key.into(),
                }])?;
                Ok(existed)
            }
            fn scan(&self, table: &str, prefix: &str) -> Result<Vec<(String, Vec<u8>)>> {
                self.inner.scan(table, prefix)
            }
            fn batch(&self, ops: Vec<Op>) -> Result<()> {
                if self.hold.load(Ordering::SeqCst) {
                    if let Some(tx) = self.entered.lock().unwrap().take() {
                        let _ = tx.send(());
                    }
                    if let Some(rx) = self.release.lock().unwrap().take() {
                        let _ = rx.recv_timeout(Duration::from_secs(5));
                    }
                }
                self.inner.batch(ops)
            }
            fn set_record_log(&self, on: bool) {
                self.inner.set_record_log(on)
            }
            fn set_log_term(&self, term: u64) {
                self.inner.set_log_term(term)
            }
            fn set_write_fence(&self, on: bool) {
                self.inner.set_write_fence(on)
            }
            fn set_applying(&self, on: bool) {
                self.inner.set_applying(on)
            }
            fn read_log(&self, after: u64, limit: usize) -> Result<Vec<LogEntry>> {
                self.inner.read_log(after, limit)
            }
            fn record_log(&self) -> bool {
                self.inner.record_log()
            }
        }

        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let backend = Arc::new(HoldBatch {
            inner: RedbBackend::in_memory().unwrap(),
            hold: AtomicBool::new(false),
            entered: Mutex::new(Some(entered_tx)),
            release: Mutex::new(Some(release_rx)),
        });
        let (events, _) = broadcast::channel(8);
        let store = Store::new(backend.clone(), events);
        store.set_log_term(5);
        store.set_record_log(true);
        store.put_meta("before", &"one".to_string()).unwrap();
        let commit = store.commit_index().unwrap();

        backend.hold.store(true, Ordering::SeqCst);
        let writer = {
            let store = store.clone();
            thread::spawn(move || store.put_meta("racer", &"split".to_string()))
        };
        entered_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("writer did not enter batch");
        store.set_write_fence(true);
        store.set_record_log(false);
        release_tx.send(()).unwrap();
        let err = writer.join().unwrap().unwrap_err();
        assert!(matches!(err, StoreError::NotLeader), "{err}");
        assert!(store.get_meta::<String>("racer").unwrap().is_none());
        assert_eq!(store.commit_index().unwrap(), commit);
    }

    #[test]
    fn write_fence_clears_when_leading_again() {
        let store = Store::in_memory().unwrap();
        store.set_write_fence(true);
        store.set_record_log(false);
        assert!(matches!(
            store.put_meta("x", &1u64).unwrap_err(),
            StoreError::NotLeader
        ));
        store.set_record_log(true);
        store.set_write_fence(false);
        store.put_meta("x", &1u64).unwrap();
        assert_eq!(store.get_meta::<u64>("x").unwrap(), Some(1));
    }

    #[test]
    fn an_interned_kind_is_the_same_pointer_twice() {
        // Equal text handed in as two distinct strings — as it arrives when read
        // off a key — must come back as one stored string, not a fresh leak.
        let owned = String::from("backup");
        let again = String::from("backup");
        let first = intern(&owned);
        let second = intern(&again);
        assert!(std::ptr::eq(first, second));
        // A different kind is a different string.
        assert!(!std::ptr::eq(intern("backup"), intern("volume")));
    }
}
