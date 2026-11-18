// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use bytes::Bytes;
use futures::Stream;
use std::path::PathBuf;
use tokio::io::AsyncReadExt;

use super::concat_state::ConcatState;

pub(crate) fn concat_parts_stream(
    parts: Vec<(PathBuf, u64)>,
) -> impl Stream<Item = Result<Bytes, std::io::Error>> + Send {
    let state = ConcatState {
        parts: parts
            .into_iter()
            .map(|(p, _)| p)
            .collect::<Vec<_>>()
            .into_iter(),
        current: None,
        buf: vec![0u8; 256 * 1024],
    };
    futures::stream::unfold(state, |mut st| async move {
        loop {
            if st.current.is_none() {
                let path = st.parts.next()?;
                match tokio::fs::File::open(&path).await {
                    Ok(f) => st.current = Some(f),
                    Err(e) => return Some((Err(e), st)),
                }
            }
            let file = st.current.as_mut().unwrap();
            match file.read(&mut st.buf).await {
                Ok(0) => {
                    st.current = None;
                    continue;
                }
                Ok(n) => {
                    let bytes = Bytes::copy_from_slice(&st.buf[..n]);
                    return Some((Ok(bytes), st));
                }
                Err(e) => return Some((Err(e), st)),
            }
        }
    })
}
