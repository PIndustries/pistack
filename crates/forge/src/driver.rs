// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The seam between the scheduler/reconciler and whatever actually runs a
//! machine on a host.
//!
//! * [`HostDriver`] is the trait the reconciler calls. One implementation per
//!   host, looked up in the [`DriverRegistry`].
//! * [`MockDriver`] is an in-memory implementation with deterministic
//!   addresses (`10.0.0.N`). Dev mode and tests use it.
//! * [`ChannelDriver`] turns every call into a [`DriverRequest`] on an
//!   in-process mpsc channel. Whoever owns the receiver (an `anvil` runtime
//!   in the same process, or the `pistack-rpc` transport later) answers via
//!   the oneshot in the request. [`serve_driver`] is the trivial pump that
//!   answers requests with any other `HostDriver`.

use crate::types::{Machine, PowerState};
use async_trait::async_trait;
use parking_lot::{Mutex, RwLock};
use pistack_types::{ApiError, ApiResult, Id};
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};

/// What a driver knows about a machine it runs.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MachineRuntime {
    pub id: Id,
    pub addresses: Vec<String>,
    pub power: PowerState,
}

#[async_trait]
pub trait HostDriver: Send + Sync + 'static {
    /// Create (if needed) and power on. Idempotent: starting a running
    /// machine returns its current runtime.
    async fn start(&self, m: &Machine) -> ApiResult<MachineRuntime>;
    /// Power off, keep the definition and disks.
    async fn stop(&self, id: &Id) -> ApiResult<()>;
    /// Tear down everything. Deleting an unknown machine is not an error.
    async fn delete(&self, id: &Id) -> ApiResult<()>;
    /// Everything this host currently runs.
    async fn list(&self) -> ApiResult<Vec<MachineRuntime>>;
}

// ---- MockDriver ------------------------------------------------------------

/// In-memory driver. Addresses are allocated from `10.0.0.1` upwards and
/// are stable for the life of a machine.
#[derive(Default)]
pub struct MockDriver {
    inner: Mutex<MockState>,
}

#[derive(Default)]
struct MockState {
    machines: BTreeMap<Id, MachineRuntime>,
    next_addr: u32,
}

impl MockDriver {
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of machines the mock knows about (running or stopped).
    pub fn len(&self) -> usize {
        self.inner.lock().machines.len()
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[async_trait]
impl HostDriver for MockDriver {
    async fn start(&self, m: &Machine) -> ApiResult<MachineRuntime> {
        let mut st = self.inner.lock();
        if let Some(rt) = st.machines.get_mut(&m.meta.id) {
            rt.power = PowerState::On;
            return Ok(rt.clone());
        }
        st.next_addr += 1;
        let n = st.next_addr;
        if n > 254 {
            return Err(ApiError::unavailable("mock driver: address pool exhausted"));
        }
        let rt = MachineRuntime {
            id: m.meta.id.clone(),
            addresses: vec![format!("10.0.0.{n}")],
            power: PowerState::On,
        };
        st.machines.insert(m.meta.id.clone(), rt.clone());
        Ok(rt)
    }

    async fn stop(&self, id: &Id) -> ApiResult<()> {
        let mut st = self.inner.lock();
        match st.machines.get_mut(id) {
            Some(rt) => {
                rt.power = PowerState::Off;
                Ok(())
            }
            None => Err(ApiError::not_found(format!(
                "mock driver: machine {id} not running here"
            ))),
        }
    }

    async fn delete(&self, id: &Id) -> ApiResult<()> {
        self.inner.lock().machines.remove(id);
        Ok(())
    }

    async fn list(&self) -> ApiResult<Vec<MachineRuntime>> {
        Ok(self.inner.lock().machines.values().cloned().collect())
    }
}

// ---- ChannelDriver ---------------------------------------------------------

/// One driver call, as a message.
#[derive(Debug)]
pub enum DriverCommand {
    Start(Box<Machine>),
    Stop(Id),
    Delete(Id),
    List,
}

/// The answer to a `DriverCommand`.
#[derive(Debug)]
pub enum DriverReply {
    Started(MachineRuntime),
    Done,
    Listed(Vec<MachineRuntime>),
}

/// A command plus the oneshot to answer it on.
#[derive(Debug)]
pub struct DriverRequest {
    pub command: DriverCommand,
    pub reply: oneshot::Sender<ApiResult<DriverReply>>,
}

/// A `HostDriver` whose calls travel over an mpsc channel. The receiving
/// side is whatever transport the deployment uses; the reconciler does not
/// care.
pub struct ChannelDriver {
    tx: mpsc::Sender<DriverRequest>,
}

impl ChannelDriver {
    /// Create the driver and the receiver its requests arrive on.
    pub fn new(capacity: usize) -> (Self, mpsc::Receiver<DriverRequest>) {
        let (tx, rx) = mpsc::channel(capacity.max(1));
        (Self { tx }, rx)
    }

    /// Wrap an existing sender (several hosts may share one queue).
    pub fn from_sender(tx: mpsc::Sender<DriverRequest>) -> Self {
        Self { tx }
    }

    async fn call(&self, command: DriverCommand) -> ApiResult<DriverReply> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(DriverRequest { command, reply })
            .await
            .map_err(|_| ApiError::unavailable("host driver channel closed"))?;
        rx.await
            .map_err(|_| ApiError::unavailable("host driver dropped the request"))?
    }
}

#[async_trait]
impl HostDriver for ChannelDriver {
    async fn start(&self, m: &Machine) -> ApiResult<MachineRuntime> {
        match self.call(DriverCommand::Start(Box::new(m.clone()))).await? {
            DriverReply::Started(rt) => Ok(rt),
            other => Err(unexpected("start", &other)),
        }
    }
    async fn stop(&self, id: &Id) -> ApiResult<()> {
        match self.call(DriverCommand::Stop(id.clone())).await? {
            DriverReply::Done => Ok(()),
            other => Err(unexpected("stop", &other)),
        }
    }
    async fn delete(&self, id: &Id) -> ApiResult<()> {
        match self.call(DriverCommand::Delete(id.clone())).await? {
            DriverReply::Done => Ok(()),
            other => Err(unexpected("delete", &other)),
        }
    }
    async fn list(&self) -> ApiResult<Vec<MachineRuntime>> {
        match self.call(DriverCommand::List).await? {
            DriverReply::Listed(v) => Ok(v),
            other => Err(unexpected("list", &other)),
        }
    }
}

fn unexpected(op: &str, got: &DriverReply) -> ApiError {
    ApiError::internal(format!("host driver: unexpected reply to {op}: {got:?}"))
}

/// Answer a `ChannelDriver`'s requests with `backend` until the sender side
/// goes away. Use it to put a local runtime behind a channel, or as the
/// reference for what the RPC transport must do.
pub async fn serve_driver(mut rx: mpsc::Receiver<DriverRequest>, backend: Arc<dyn HostDriver>) {
    while let Some(req) = rx.recv().await {
        let result = match req.command {
            DriverCommand::Start(m) => backend.start(&m).await.map(DriverReply::Started),
            DriverCommand::Stop(id) => backend.stop(&id).await.map(|_| DriverReply::Done),
            DriverCommand::Delete(id) => backend.delete(&id).await.map(|_| DriverReply::Done),
            DriverCommand::List => backend.list().await.map(DriverReply::Listed),
        };
        // The caller gave up; nothing to do.
        let _ = req.reply.send(result);
    }
}

// ---- DriverRegistry --------------------------------------------------------

/// Host id -> driver. Hosts marked `local` are served in-process (no agent
/// heartbeat), so the reconciler keeps their `last_seen` fresh itself.
#[derive(Default)]
pub struct DriverRegistry {
    inner: RwLock<HashMap<Id, Entry>>,
}

struct Entry {
    driver: Arc<dyn HostDriver>,
    local: bool,
}

impl DriverRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register the driver that reaches `host`. Replaces any previous one.
    pub fn register(&self, host: Id, driver: Arc<dyn HostDriver>) {
        self.inner.write().insert(
            host,
            Entry {
                driver,
                local: false,
            },
        );
    }

    /// Register a driver that lives in this process (dev mode). The
    /// reconciler heartbeats these hosts itself.
    pub fn register_local(&self, host: Id, driver: Arc<dyn HostDriver>) {
        self.inner.write().insert(
            host,
            Entry {
                driver,
                local: true,
            },
        );
    }

    pub fn unregister(&self, host: &Id) -> bool {
        self.inner.write().remove(host).is_some()
    }

    pub fn get(&self, host: &Id) -> Option<Arc<dyn HostDriver>> {
        self.inner.read().get(host).map(|e| e.driver.clone())
    }

    pub fn is_local(&self, host: &Id) -> bool {
        self.inner
            .read()
            .get(host)
            .map(|e| e.local)
            .unwrap_or(false)
    }

    pub fn hosts(&self) -> Vec<Id> {
        self.inner.read().keys().cloned().collect()
    }

    pub fn len(&self) -> usize {
        self.inner.read().len()
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{new_machine, MachineSpec};

    fn machine(name: &str) -> Machine {
        new_machine(
            name,
            None,
            MachineSpec {
                size: "s.1x1".into(),
                image: "x".into(),
                ..Default::default()
            },
        )
    }

    #[tokio::test]
    async fn mock_addresses_are_deterministic_and_stable() {
        let d = MockDriver::new();
        let a = machine("a");
        let b = machine("b");
        assert_eq!(d.start(&a).await.unwrap().addresses, vec!["10.0.0.1"]);
        assert_eq!(d.start(&b).await.unwrap().addresses, vec!["10.0.0.2"]);
        // restart keeps the address
        assert_eq!(d.start(&a).await.unwrap().addresses, vec!["10.0.0.1"]);
        d.stop(&a.meta.id).await.unwrap();
        assert_eq!(
            d.list()
                .await
                .unwrap()
                .iter()
                .filter(|r| r.power == PowerState::Off)
                .count(),
            1
        );
        d.delete(&a.meta.id).await.unwrap();
        d.delete(&a.meta.id).await.unwrap(); // idempotent
        assert_eq!(d.len(), 1);
        assert!(d.stop(&a.meta.id).await.is_err());
    }

    #[tokio::test]
    async fn channel_driver_round_trips() {
        let (driver, rx) = ChannelDriver::new(8);
        let backend: Arc<dyn HostDriver> = Arc::new(MockDriver::new());
        let pump = tokio::spawn(serve_driver(rx, backend));
        let m = machine("c");
        let rt = driver.start(&m).await.unwrap();
        assert_eq!(rt.addresses, vec!["10.0.0.1"]);
        assert_eq!(driver.list().await.unwrap().len(), 1);
        driver.stop(&m.meta.id).await.unwrap();
        driver.delete(&m.meta.id).await.unwrap();
        assert!(driver.list().await.unwrap().is_empty());
        drop(driver);
        pump.await.unwrap();
    }

    #[tokio::test]
    async fn channel_driver_reports_closed_receiver() {
        let (driver, rx) = ChannelDriver::new(1);
        drop(rx);
        let err = driver.list().await.unwrap_err();
        assert_eq!(err.code, pistack_types::error::ErrorCode::Unavailable);
    }
}
