// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The PiStack state store.
//!
//! One embedded, transactional KV (redb) holds every resource of every
//! module, keyed `{kind}/{id}`. Modules use the typed `Collection<K>` API and
//! never touch bytes. The `Backend` trait is the seam where `openraft`
//! replication lands later: same API, replicated log underneath.
//!
//! Values are JSON. Human-inspectable with `pistack store dump`, and the
//! schema can evolve with serde defaults instead of migrations.

use parking_lot::RwLock;
use pistack_types::{ApiError, Event, EventAction, Id, Kind, Meta};
use redb::{Database, ReadableTable, TableDefinition};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::broadcast;

const RESOURCES: TableDefinition<&str, &[u8]> = TableDefinition::new("resources");
/// Secondary index: `{kind}/{project}/{name}` -> id, enforcing unique names
/// per project per kind and giving fast lookup by name.
const NAMES: TableDefinition<&str, &str> = TableDefinition::new("names");
/// Misc small values (cluster id, key material refs, counters).
const META: TableDefinition<&str, &[u8]> = TableDefinition::new("meta");

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
                "cluster_term" | "cluster_voted_for" | "cluster_leader" | "node_id" | LOG_START_KEY
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

fn log_key(index: u64) -> String {
    format!("cluster_log/{index:020}")
}

/// First log row this store still keeps. Rows below it were trimmed by
/// [`Store::compact_log`]; the state they produced is in the tables.
const LOG_START_KEY: &str = "cluster_log_start";

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
        let db = Database::create(path)?;
        // Ensure tables exist.
        let tx = db.begin_write()?;
        {
            tx.open_table(RESOURCES)?;
            tx.open_table(NAMES)?;
            tx.open_table(META)?;
        }
        tx.commit()?;
        Ok(Self {
            db,
            record: AtomicBool::new(false),
            term: AtomicU64::new(0),
            fenced: AtomicBool::new(false),
            applying: AtomicBool::new(false),
            gate: parking_lot::Mutex::new(()),
        })
    }

    pub fn in_memory() -> Result<Self> {
        let db = Database::builder().create_with_backend(redb::backends::InMemoryBackend::new())?;
        let tx = db.begin_write()?;
        {
            tx.open_table(RESOURCES)?;
            tx.open_table(NAMES)?;
            tx.open_table(META)?;
        }
        tx.commit()?;
        Ok(Self {
            db,
            record: AtomicBool::new(false),
            term: AtomicU64::new(0),
            fenced: AtomicBool::new(false),
            applying: AtomicBool::new(false),
            gate: parking_lot::Mutex::new(()),
        })
    }

    fn table_def(name: &str) -> Result<TableDefinition<'static, &'static str, &'static [u8]>> {
        match name {
            "resources" => Ok(RESOURCES),
            "meta" => Ok(META),
            other => Err(StoreError::Storage(format!("unknown table {other}"))),
        }
    }
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

    /// Delete one meta key.
    pub fn delete_meta(&self, key: &str) -> Result<bool> {
        self.backend.delete("meta", key)
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
    pub fn apply_entries(&self, entries: &[LogEntry]) -> Result<()> {
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
        result
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
        if let Some(id) = Id::parse(leak(K::KIND), name_or_id) {
            return self.get(&id);
        }
        self.get_by_name(project, name_or_id)
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
        // Rename support: move the name index entry.
        if current.meta().name != item.meta().name {
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

fn leak(s: &str) -> &'static str {
    // KIND constants are already 'static; this only runs for them.
    Box::leak(s.to_owned().into_boxed_str())
}

/// Copy a redb file and open the copy.
///
/// The write lock is held for the copy, so the bytes stay at the last commit.
/// `dst` must not already exist.
pub fn backup_file(src: &Path, dst: &Path) -> Result<()> {
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
    match Database::open(dst) {
        Ok(check) => {
            let _ = check.begin_read()?;
            Ok(())
        }
        Err(e) => {
            let _ = std::fs::remove_file(dst);
            Err(e.into())
        }
    }
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
}
