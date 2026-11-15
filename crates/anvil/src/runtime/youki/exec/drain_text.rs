// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::time::Duration;

pub(in crate::runtime::youki) async fn drain_text(task: tokio::task::JoinHandle<String>) -> String {
    match tokio::time::timeout(Duration::from_secs(2), task).await {
        Ok(Ok(text)) => text,
        _ => String::new(),
    }
}
