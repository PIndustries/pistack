// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::sync::Arc;

use tokio::sync::mpsc;

use crate::proto::Command;

use super::node_info::NodeInfo;

pub(crate) struct NodeEntry {
    pub(crate) tx: mpsc::Sender<Command>,
    pub(crate) rx: Arc<tokio::sync::Mutex<mpsc::Receiver<Command>>>,
    pub(crate) info: NodeInfo,
}
