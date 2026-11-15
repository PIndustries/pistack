// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::consts::MAX_IMAGE_BYTES;
use crate::runtime::youki::image::cannot_unpack;
use crate::runtime::youki::image::checksum_ok;
use crate::runtime::youki::image::cstr;
use crate::runtime::youki::image::parse_number;
use crate::runtime::youki::image::parse_pax;
use crate::runtime::youki::image::tar_name;
use pistack_types::ApiResult;
use std::io::Read;
use std::io::Write;

pub(in crate::runtime::youki) struct TarReader<R: Read> {
    pub(in crate::runtime::youki) inner: R,
    pub(in crate::runtime::youki) pending_name: Option<String>,
    pub(in crate::runtime::youki) pending_link: Option<String>,
}

pub(in crate::runtime::youki) struct TarEntry {
    pub(in crate::runtime::youki) name: String,
    pub(in crate::runtime::youki) link: String,
    pub(in crate::runtime::youki) size: u64,
    pub(in crate::runtime::youki) flag: u8,
    pub(in crate::runtime::youki) mode: u32,
}

impl<R: Read> TarReader<R> {
    pub(in crate::runtime::youki) fn next_entry(&mut self) -> ApiResult<Option<TarEntry>> {
        loop {
            let mut header = [0u8; 512];
            match self.inner.read_exact(&mut header) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
                Err(_) => return Err(cannot_unpack()),
            }
            if header.iter().all(|b| *b == 0) {
                return Ok(None);
            }
            if !checksum_ok(&header) {
                return Err(cannot_unpack());
            }
            let flag = header[156];
            let mode = parse_number(&header[100..108]).ok_or_else(cannot_unpack)?;
            let mode = u32::try_from(mode & 0o7777).unwrap_or(0);
            let size = parse_number(&header[124..136]).ok_or_else(cannot_unpack)?;
            if size > MAX_IMAGE_BYTES {
                return Err(cannot_unpack());
            }
            let mut name = tar_name(&header);
            let mut link = cstr(&header[157..257]);
            match flag {
                b'L' => {
                    self.pending_name = Some(self.read_string(size)?);
                    continue;
                }
                b'K' => {
                    self.pending_link = Some(self.read_string(size)?);
                    continue;
                }
                b'x' | b'g' => {
                    let text = self.read_bytes(size)?;
                    let (path, linkpath) = parse_pax(&text);
                    if let Some(path) = path {
                        self.pending_name = Some(path);
                    }
                    if let Some(linkpath) = linkpath {
                        self.pending_link = Some(linkpath);
                    }
                    continue;
                }
                _ => {}
            }
            if let Some(over) = self.pending_name.take() {
                name = over;
            }
            if let Some(over) = self.pending_link.take() {
                link = over;
            }
            return Ok(Some(TarEntry {
                name,
                link,
                size,
                flag,
                mode,
            }));
        }
    }

    pub(in crate::runtime::youki) fn read_string(&mut self, size: u64) -> ApiResult<String> {
        let bytes = self.read_bytes(size)?;
        let end = bytes.iter().position(|b| *b == 0).unwrap_or(bytes.len());
        Ok(String::from_utf8_lossy(&bytes[..end]).into_owned())
    }

    pub(in crate::runtime::youki) fn read_bytes(&mut self, size: u64) -> ApiResult<Vec<u8>> {
        if size > MAX_IMAGE_BYTES {
            return Err(cannot_unpack());
        }
        let mut buf = vec![0u8; size as usize];
        self.inner
            .read_exact(&mut buf)
            .map_err(|_| cannot_unpack())?;
        self.skip_pad(size)?;
        Ok(buf)
    }

    pub(in crate::runtime::youki) fn copy_entry(
        &mut self,
        size: u64,
        mut out: impl Write,
        budget: &mut u64,
    ) -> ApiResult<()> {
        if size > *budget {
            return Err(cannot_unpack());
        }
        *budget -= size;
        let mut left = size;
        let mut buf = [0u8; 8192];
        while left > 0 {
            let n = left.min(buf.len() as u64) as usize;
            self.inner
                .read_exact(&mut buf[..n])
                .map_err(|_| cannot_unpack())?;
            out.write_all(&buf[..n]).map_err(|_| cannot_unpack())?;
            left -= n as u64;
        }
        self.skip_pad(size)
    }

    pub(in crate::runtime::youki) fn discard(&mut self, size: u64) -> ApiResult<()> {
        let mut left = size;
        let mut buf = [0u8; 8192];
        while left > 0 {
            let n = left.min(buf.len() as u64) as usize;
            self.inner
                .read_exact(&mut buf[..n])
                .map_err(|_| cannot_unpack())?;
            left -= n as u64;
        }
        self.skip_pad(size)
    }

    pub(in crate::runtime::youki) fn skip_pad(&mut self, size: u64) -> ApiResult<()> {
        let pad = ((512 - (size % 512)) % 512) as usize;
        if pad == 0 {
            return Ok(());
        }
        let mut buf = vec![0u8; pad];
        self.inner
            .read_exact(&mut buf)
            .map_err(|_| cannot_unpack())?;
        Ok(())
    }
}
