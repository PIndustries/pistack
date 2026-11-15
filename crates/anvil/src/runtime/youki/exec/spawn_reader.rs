// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::exec::bytes_to_text;
use crate::runtime::youki::exec::read_capped;

pub(in crate::runtime::youki) fn spawn_reader<R>(
    pipe: Option<R>,
    cap: usize,
) -> tokio::task::JoinHandle<String>
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        match pipe {
            Some(pipe) => bytes_to_text(&read_capped(pipe, cap).await),
            None => String::new(),
        }
    })
}
