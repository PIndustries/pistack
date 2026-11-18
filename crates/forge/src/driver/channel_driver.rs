// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::Machine;
use async_trait::async_trait;
use pistack_types::{ApiError, ApiResult, Id};
use tokio::sync::{mpsc, oneshot};

use super::driver_command::DriverCommand;
use super::driver_reply::DriverReply;
use super::driver_request::DriverRequest;
use super::host_driver::HostDriver;
use super::machine_runtime::MachineRuntime;
use super::unexpected::unexpected;

/// A `HostDriver` whose calls travel over an mpsc channel. The receiving
/// side is whatever transport the deployment uses; the reconciler does not
/// care.
pub struct ChannelDriver {
    pub(crate) tx: mpsc::Sender<DriverRequest>,
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

    pub(crate) async fn call(&self, command: DriverCommand) -> ApiResult<DriverReply> {
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
    async fn rebuild(&self, m: &Machine) -> ApiResult<MachineRuntime> {
        match self
            .call(DriverCommand::Rebuild(Box::new(m.clone())))
            .await?
        {
            DriverReply::Started(rt) => Ok(rt),
            other => Err(unexpected("rebuild", &other)),
        }
    }
}
