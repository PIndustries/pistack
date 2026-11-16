// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_store::{Backend, LogEntry, Op, RedbBackend, Store};
use pistack_types::Id;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::backend::cache::forget_unwrapped;
use crate::backend::root_key_state::{root_key_state, RootKeyState};
use crate::backend::KeyBackend;
use crate::client::Client;
use crate::crypto::RootKey;
use crate::types::{Secret, SecretSpec};

/// A store whose next transaction can be made to "crash": the batch is
/// dropped before commit, as a power cut would drop it.
pub(in crate::backend) struct Crashy {
    inner: RedbBackend,
    pub(in crate::backend) crash: AtomicBool,
}

impl Backend for Crashy {
    fn get(&self, t: &str, k: &str) -> pistack_store::Result<Option<Vec<u8>>> {
        self.inner.get(t, k)
    }
    fn put(&self, t: &str, k: &str, v: &[u8]) -> pistack_store::Result<()> {
        self.batch(vec![Op::Put {
            table: t.into(),
            key: k.into(),
            value: v.to_vec(),
        }])
    }
    fn delete(&self, t: &str, k: &str) -> pistack_store::Result<bool> {
        let had = self.inner.get(t, k)?.is_some();
        self.batch(vec![Op::Delete {
            table: t.into(),
            key: k.into(),
        }])?;
        Ok(had)
    }
    fn scan(&self, t: &str, p: &str) -> pistack_store::Result<Vec<(String, Vec<u8>)>> {
        self.inner.scan(t, p)
    }
    fn batch(&self, ops: Vec<Op>) -> pistack_store::Result<()> {
        if self.crash.load(Ordering::SeqCst) {
            return Err(pistack_store::StoreError::Storage(
                "simulated crash before commit".into(),
            ));
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
    fn read_log(&self, after: u64, limit: usize) -> pistack_store::Result<Vec<LogEntry>> {
        self.inner.read_log(after, limit)
    }
    fn record_log(&self) -> bool {
        self.inner.record_log()
    }
}

pub(in crate::backend) fn crashy_store() -> (Store, Arc<Crashy>) {
    let b = Arc::new(Crashy {
        inner: RedbBackend::in_memory().unwrap(),
        crash: AtomicBool::new(false),
    });
    let (tx, _) = tokio::sync::broadcast::channel(16);
    (Store::new(b.clone(), tx), b)
}

pub(in crate::backend) fn state(store: &Store) -> RootKeyState {
    root_key_state(store).unwrap()
}

/// A software store with one secret and the CA, as a running cloud has.
pub(in crate::backend) fn software_cloud(store: &Store) -> (RootKey, Id, String) {
    let c = Client::open_with(store, &KeyBackend::Software).unwrap();
    let s = c
        .create_secret(Secret::new("db", None, SecretSpec::default()), b"pw")
        .unwrap();
    let key = KeyBackend::Software.load_existing(store).unwrap().unwrap();
    (key, s.meta.id, c.ca_bundle_pem())
}

pub(in crate::backend) fn reopen_and_check(
    store: &Store,
    backend: &KeyBackend,
    secret: &Id,
    ca_pem: &str,
) {
    forget_unwrapped();
    let c = Client::open_with(store, backend).unwrap();
    assert_eq!(c.get_secret_payload(secret).unwrap(), b"pw");
    assert_eq!(c.ca_bundle_pem(), ca_pem);
}
