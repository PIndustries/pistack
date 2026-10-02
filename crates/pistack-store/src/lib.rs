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
use redb::{Database, TableDefinition};
use serde::{de::DeserializeOwned, Serialize};
use std::collections::BTreeMap;
use std::path::Path;
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
    #[error("storage: {0}")]
    Storage(String),
    #[error("codec: {0}")]
    Codec(#[from] serde_json::Error),
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
}

#[derive(Debug, Clone)]
pub enum Op {
    Put { table: String, key: String, value: Vec<u8> },
    Delete { table: String, key: String },
}

/// redb-backed `Backend`.
pub struct RedbBackend {
    db: Database,
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
        Ok(Self { db })
    }

    pub fn in_memory() -> Result<Self> {
        let db = Database::builder()
            .create_with_backend(redb::backends::InMemoryBackend::new())?;
        let tx = db.begin_write()?;
        {
            tx.open_table(RESOURCES)?;
            tx.open_table(NAMES)?;
            tx.open_table(META)?;
        }
        tx.commit()?;
        Ok(Self { db })
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
        self.batch(vec![Op::Put { table: table.into(), key: key.into(), value: value.to_vec() }])
    }

    fn delete(&self, table: &str, key: &str) -> Result<bool> {
        let existed = self.get(table, key)?.is_some();
        self.batch(vec![Op::Delete { table: table.into(), key: key.into() }])?;
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
        let tx = self.db.begin_write()?;
        {
            let mut res = tx.open_table(RESOURCES)?;
            let mut names = tx.open_table(NAMES)?;
            let mut meta = tx.open_table(META)?;
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
        }
        tx.commit()?;
        Ok(())
    }
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
        Self { backend, events, write_lock: Arc::new(RwLock::new(())) }
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
        Collection { store: self.clone(), _k: std::marker::PhantomData }
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
        Self { store: self.store.clone(), _k: std::marker::PhantomData }
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
            .filter(|k| labels.iter().all(|(lk, lv)| k.meta().labels.get(lk) == Some(lv)))
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
            Op::Put { table: "resources".into(), key: rk, value: serde_json::to_vec(&item)? },
            Op::Put { table: "names".into(), key: nk, value: meta.id.to_string().into_bytes() },
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
                return Err(StoreError::Revision { expected: exp, found: cur_rev });
            }
        }
        let mut ops = Vec::new();
        // Rename support: move the name index entry.
        if current.meta().name != item.meta().name {
            let old = Store::name_key(K::KIND, current.meta().project.as_ref(), &current.meta().name);
            let new = Store::name_key(K::KIND, item.meta().project.as_ref(), &item.meta().name);
            if self.store.backend.get("names", &new)?.is_some() {
                return Err(StoreError::Conflict(format!(
                    "{} named '{}' already exists",
                    K::KIND,
                    item.meta().name
                )));
            }
            ops.push(Op::Delete { table: "names".into(), key: old });
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
        Err(StoreError::Conflict(format!("{} {id}: too many concurrent writers", K::KIND)))
    }

    pub fn delete(&self, id: &Id) -> Result<K> {
        let _g = self.store.write_lock.write();
        let cur = self.get(id)?;
        let meta = cur.meta().clone();
        self.store.backend.batch(vec![
            Op::Delete { table: "resources".into(), key: Store::res_key(K::KIND, id) },
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

/// Re-export so modules can name the shared kinds without importing types.
pub use pistack_types::kind as kinds;

#[cfg(test)]
mod tests {
    use super::*;
    use pistack_types::{kind, Meta, Resource};
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
    impl Kind for Widget {
        const KIND: &'static str = "widget";
        fn meta(&self) -> &Meta {
            &self.meta
        }
        fn meta_mut(&mut self) -> &mut Meta {
            &mut self.meta
        }
    }

    fn widget(name: &str, project: Option<Id>) -> Widget {
        Widget {
            meta: Meta::new("widget", name, project),
            spec: WidgetSpec { size: 1 },
            status: WidgetStatus::default(),
        }
    }

    #[test]
    fn crud_and_names() {
        let store = Store::in_memory().unwrap();
        let col = store.collection::<Widget>();
        let proj = Id::new(kind::PROJECT);
        let w = col.create(widget("a", Some(proj.clone()))).unwrap();
        assert_eq!(col.get(&w.meta.id).unwrap().spec.size, 1);
        assert_eq!(col.get_by_name(Some(&proj), "a").unwrap().meta.id, w.meta.id);
        assert!(matches!(
            col.create(widget("a", Some(proj.clone()))),
            Err(StoreError::Conflict(_))
        ));
        // same name in another project is fine
        let other = Id::new(kind::PROJECT);
        col.create(widget("a", Some(other.clone()))).unwrap();
        assert_eq!(col.list(Some(&proj)).unwrap().len(), 1);
        assert_eq!(col.list(None).unwrap().len(), 2);

        let updated = col.modify(&w.meta.id, |x| { x.spec.size = 5; Ok(()) }).unwrap();
        assert_eq!(updated.meta.revision, 2);
        assert!(matches!(col.update(w.clone(), Some(1)), Err(StoreError::Revision { .. })));

        col.delete(&w.meta.id).unwrap();
        assert!(matches!(col.get(&w.meta.id), Err(StoreError::NotFound(_))));
        assert!(matches!(col.get_by_name(Some(&proj), "a"), Err(StoreError::NotFound(_))));
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
            s.collection::<Widget>().create(widget("disk", None)).unwrap();
            s.put_meta("cluster_id", &"abc").unwrap();
        }
        let s = Store::open(&path, tx).unwrap();
        assert_eq!(s.collection::<Widget>().list(None).unwrap().len(), 1);
        assert_eq!(s.get_meta::<String>("cluster_id").unwrap().unwrap(), "abc");
    }
}
