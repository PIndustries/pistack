// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::sync::Arc;
use tokio::sync::mpsc;

use super::driver_command::DriverCommand;
use super::driver_reply::DriverReply;
use super::driver_request::DriverRequest;
use super::host_driver::HostDriver;

/// Answer a `ChannelDriver`'s requests with `backend` until the sender side
/// goes away. Use it to put a local runtime behind a channel, or as the
/// reference for what the RPC transport must do.
pub async fn serve_driver(mut rx: mpsc::Receiver<DriverRequest>, backend: Arc<dyn HostDriver>) {
    while let Some(req) = rx.recv().await {
        let result = match req.command {
            DriverCommand::Start(m) => backend.start(&m).await.map(DriverReply::Started),
            DriverCommand::Stop(id) => backend.stop(&id).await.map(|_| DriverReply::Done),
            DriverCommand::Delete(id) => backend.delete(&id).await.map(|_| DriverReply::Done),
            DriverCommand::Rebuild(m) => backend.rebuild(&m).await.map(DriverReply::Started),
            DriverCommand::List => backend.list().await.map(DriverReply::Listed),
        };
        // The caller gave up; nothing to do.
        let _ = req.reply.send(result);
    }
}
