// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Look inside an uploaded disk image.
//!
//! With `qemu-img` on the server: `qemu-img info --output=json -f <fmt>`
//! (the declared format is forced so qemu never probes; probing is how a
//! raw image that *looks like* qcow2 gets a backing file pointed at a
//! host path). Without it: read the first 512 bytes and parse the qcow2
//! header ourselves. Either way an image that names a backing file or an
//! external data file is rejected: it would make the hypervisor open an
//! arbitrary file on the compute host.

use pistack_types::{ApiError, ApiResult};
use serde::Deserialize;
use std::path::Path;
use tokio::io::AsyncReadExt;

use crate::store::ImageData;
use crate::types::ImageFormat;

pub const QCOW2_MAGIC: &[u8; 4] = b"QFI\xfb";

/// What the bytes say they are.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DetectedFormat {
    Qcow2,
    /// No recognised header; raw disks have none.
    Raw,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Inspection {
    pub format: DetectedFormat,
    /// Disk size the image presents to the guest. `None` when unknown.
    pub virtual_size: Option<u64>,
    pub backing_file: Option<String>,
    pub data_file: Option<String>,
    /// `qemu-img` or `header`.
    pub source: &'static str,
}

impl Inspection {
    /// The security check. Backing files and external data files are
    /// never allowed in uploaded images.
    pub fn reject_unsafe(&self) -> ApiResult<()> {
        if let Some(b) = &self.backing_file {
            return Err(ApiError::unprocessable(format!(
                "image has a backing file ('{b}'); upload a flattened image (qemu-img convert -O qcow2)"
            )));
        }
        if let Some(d) = &self.data_file {
            return Err(ApiError::unprocessable(format!(
                "image has an external data file ('{d}'); not allowed"
            )));
        }
        Ok(())
    }
}

// ---- qemu-img -------------------------------------------------------------

#[derive(Deserialize)]
struct QemuInfo {
    #[serde(rename = "virtual-size")]
    virtual_size: Option<u64>,
    format: Option<String>,
    #[serde(rename = "backing-filename")]
    backing_filename: Option<String>,
    #[serde(rename = "full-backing-filename")]
    full_backing_filename: Option<String>,
    #[serde(rename = "format-specific")]
    format_specific: Option<QemuFormatSpecific>,
}

#[derive(Deserialize)]
struct QemuFormatSpecific {
    data: Option<QemuFormatData>,
}

#[derive(Deserialize)]
struct QemuFormatData {
    #[serde(rename = "data-file")]
    data_file: Option<String>,
}

/// Parse `qemu-img info --output=json`.
pub fn parse_qemu_img_info(json: &str) -> ApiResult<Inspection> {
    let info: QemuInfo = serde_json::from_str(json)
        .map_err(|e| ApiError::internal(format!("qemu-img output: {e}")))?;
    let format = match info.format.as_deref() {
        Some("qcow2") => DetectedFormat::Qcow2,
        Some("raw") | None => DetectedFormat::Raw,
        Some(other) => {
            return Err(ApiError::unprocessable(format!(
                "image format '{other}' is not supported; use raw or qcow2"
            )))
        }
    };
    let backing_file = info
        .backing_filename
        .filter(|s| !s.is_empty())
        .or(info.full_backing_filename.filter(|s| !s.is_empty()));
    let data_file = info
        .format_specific
        .and_then(|f| f.data)
        .and_then(|d| d.data_file)
        .filter(|s| !s.is_empty());
    Ok(Inspection {
        format,
        virtual_size: info.virtual_size,
        backing_file,
        data_file,
        source: "qemu-img",
    })
}

/// Is `qemu-img` on `PATH`?
pub fn qemu_img_available() -> bool {
    std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|d| d.join("qemu-img").is_file()))
        .unwrap_or(false)
}

async fn run_qemu_img(path: &Path, declared: ImageFormat) -> ApiResult<Inspection> {
    let out = tokio::process::Command::new("qemu-img")
        .arg("info")
        .arg("--output=json")
        .arg("-f")
        .arg(declared.to_string())
        .arg(path)
        .output()
        .await
        .map_err(|e| ApiError::internal(format!("running qemu-img: {e}")))?;
    if !out.status.success() {
        return Err(ApiError::unprocessable(format!(
            "qemu-img could not read the image as {declared}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    parse_qemu_img_info(&String::from_utf8_lossy(&out.stdout))
}

// ---- header parsing -------------------------------------------------------

/// Minimal qcow2 header (big-endian):
/// `magic[4] version:u32 backing_file_offset:u64 backing_file_size:u32
/// cluster_bits:u32 size:u64`.
pub fn detect_magic(head: &[u8]) -> Inspection {
    if head.len() < 4 || &head[..4] != QCOW2_MAGIC {
        return Inspection {
            format: DetectedFormat::Raw,
            virtual_size: None,
            backing_file: None,
            data_file: None,
            source: "header",
        };
    }
    let u64_at = |o: usize| {
        head.get(o..o + 8)
            .map(|b| u64::from_be_bytes(b.try_into().unwrap_or([0; 8])))
    };
    let u32_at = |o: usize| {
        head.get(o..o + 4)
            .map(|b| u32::from_be_bytes(b.try_into().unwrap_or([0; 4])))
    };
    let backing_offset = u64_at(8).unwrap_or(0);
    let backing_size = u32_at(16).unwrap_or(0);
    let virtual_size = u64_at(24);
    // We cannot read the backing name without the whole header region; its
    // presence is all the rejection needs.
    let backing_file = (backing_offset != 0 && backing_size != 0).then(|| {
        let start = backing_offset as usize;
        let end = start.saturating_add(backing_size as usize);
        head.get(start..end)
            .map(|b| String::from_utf8_lossy(b).into_owned())
            .unwrap_or_else(|| format!("<{backing_size} bytes at offset {backing_offset}>"))
    });
    // Incompatible feature bit 2 (version 3+) = external data file.
    let data_file = (u32_at(4).unwrap_or(2) >= 3 && u64_at(72).unwrap_or(0) & (1 << 2) != 0)
        .then(|| "<external data file flag set>".to_owned());
    Inspection {
        format: DetectedFormat::Qcow2,
        virtual_size,
        backing_file,
        data_file,
        source: "header",
    }
}

/// Inspect a stored blob: `qemu-img` when present and the blob is a local
/// file, else the header read from `data`.
pub async fn inspect(
    local_path: Option<&Path>,
    data: Option<ImageData>,
    declared: ImageFormat,
) -> ApiResult<Inspection> {
    if let (Some(p), true) = (local_path, qemu_img_available()) {
        let mut insp = run_qemu_img(p, declared).await?;
        // qemu was told the format, so confirm with the magic bytes too.
        if let Ok(mut f) = tokio::fs::File::open(p).await {
            let mut head = [0u8; 512];
            let n = read_head(&mut f, &mut head).await;
            let by_magic = detect_magic(&head[..n]);
            if by_magic.format == DetectedFormat::Qcow2 && insp.format == DetectedFormat::Raw {
                insp.format = DetectedFormat::Qcow2;
            }
        }
        return Ok(insp);
    }
    let mut data: ImageData =
        match (data, local_path) {
            (Some(d), _) => d,
            (None, Some(p)) => Box::pin(tokio::fs::File::open(p).await.map_err(|e| {
                ApiError::internal(format!("inspect: opening {}: {e}", p.display()))
            })?),
            (None, None) => return Err(ApiError::internal("inspect: no data to read")),
        };
    let mut head = vec![0u8; 4096];
    let n = read_head(&mut data, &mut head).await;
    Ok(detect_magic(&head[..n]))
}

async fn read_head<R: tokio::io::AsyncRead + Unpin>(r: &mut R, buf: &mut [u8]) -> usize {
    let mut filled = 0;
    while filled < buf.len() {
        match r.read(&mut buf[filled..]).await {
            Ok(0) | Err(_) => break,
            Ok(n) => filled += n,
        }
    }
    filled
}

/// Compare what was declared with what was found.
pub fn check_declared(declared: ImageFormat, found: &Inspection) -> ApiResult<()> {
    match (declared, found.format) {
        (ImageFormat::Qcow2, DetectedFormat::Qcow2) | (ImageFormat::Raw, DetectedFormat::Raw) => {
            Ok(())
        }
        (ImageFormat::Qcow2, DetectedFormat::Raw) => Err(ApiError::unprocessable(
            "image declared qcow2 but has no qcow2 header",
        )),
        (ImageFormat::Raw, DetectedFormat::Qcow2) => Err(ApiError::unprocessable(
            "image declared raw but carries a qcow2 header; declare it qcow2 or convert it",
        )),
        (ImageFormat::Oci, _) => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE_CLEAN: &str = r#"{
        "virtual-size": 2361393152,
        "filename": "ubuntu.qcow2",
        "cluster-size": 65536,
        "format": "qcow2",
        "actual-size": 591396864,
        "format-specific": {
            "type": "qcow2",
            "data": { "compat": "1.1", "compression-type": "zlib", "lazy-refcounts": false,
                      "refcount-bits": 16, "corrupt": false, "extended-l2": false }
        },
        "dirty-flag": false
    }"#;

    const FIXTURE_BACKING: &str = r#"{
        "virtual-size": 10737418240,
        "filename": "overlay.qcow2",
        "format": "qcow2",
        "actual-size": 200704,
        "backing-filename": "/etc/passwd",
        "full-backing-filename": "/etc/passwd",
        "backing-filename-format": "raw",
        "format-specific": { "type": "qcow2", "data": { "compat": "1.1" } }
    }"#;

    const FIXTURE_DATA_FILE: &str = r#"{
        "virtual-size": 1073741824, "format": "qcow2", "actual-size": 0,
        "format-specific": { "type": "qcow2", "data": { "compat": "1.1", "data-file": "/dev/sda" } }
    }"#;

    #[test]
    fn parses_clean_qcow2() {
        let i = parse_qemu_img_info(FIXTURE_CLEAN).expect("parse");
        assert_eq!(i.format, DetectedFormat::Qcow2);
        assert_eq!(i.virtual_size, Some(2_361_393_152));
        assert!(i.backing_file.is_none());
        assert!(i.data_file.is_none());
        i.reject_unsafe().expect("clean image is accepted");
        check_declared(ImageFormat::Qcow2, &i).expect("declared qcow2");
        assert!(check_declared(ImageFormat::Raw, &i).is_err());
    }

    #[test]
    fn rejects_backing_file() {
        let i = parse_qemu_img_info(FIXTURE_BACKING).expect("parse");
        assert_eq!(i.backing_file.as_deref(), Some("/etc/passwd"));
        let e = i
            .reject_unsafe()
            .expect_err("backing file must be rejected");
        assert_eq!(e.code, pistack_types::error::ErrorCode::Unprocessable);
    }

    #[test]
    fn rejects_external_data_file() {
        let i = parse_qemu_img_info(FIXTURE_DATA_FILE).expect("parse");
        assert_eq!(i.data_file.as_deref(), Some("/dev/sda"));
        assert!(i.reject_unsafe().is_err());
    }

    #[test]
    fn parses_raw_and_rejects_unknown_formats() {
        let i = parse_qemu_img_info(r#"{"virtual-size": 4096, "format": "raw"}"#).expect("parse");
        assert_eq!(i.format, DetectedFormat::Raw);
        assert!(parse_qemu_img_info(r#"{"format": "vmdk"}"#).is_err());
        assert!(parse_qemu_img_info("not json").is_err());
    }

    fn qcow2_header(backing_offset: u64, backing_size: u32, size: u64) -> Vec<u8> {
        let mut h = vec![0u8; 512];
        h[..4].copy_from_slice(QCOW2_MAGIC);
        h[4..8].copy_from_slice(&3u32.to_be_bytes());
        h[8..16].copy_from_slice(&backing_offset.to_be_bytes());
        h[16..20].copy_from_slice(&backing_size.to_be_bytes());
        h[20..24].copy_from_slice(&16u32.to_be_bytes());
        h[24..32].copy_from_slice(&size.to_be_bytes());
        h
    }

    #[test]
    fn magic_detects_qcow2_and_size() {
        let h = qcow2_header(0, 0, 8 << 30);
        let i = detect_magic(&h);
        assert_eq!(i.format, DetectedFormat::Qcow2);
        assert_eq!(i.virtual_size, Some(8 << 30));
        assert!(i.backing_file.is_none());
        assert_eq!(i.source, "header");
        i.reject_unsafe().expect("clean");
    }

    #[test]
    fn magic_detects_backing_file_in_header() {
        let mut h = qcow2_header(104, 11, 1 << 30);
        h[104..115].copy_from_slice(b"/etc/shadow");
        let i = detect_magic(&h);
        assert_eq!(i.backing_file.as_deref(), Some("/etc/shadow"));
        assert!(i.reject_unsafe().is_err());
    }

    #[test]
    fn magic_treats_everything_else_as_raw() {
        let i = detect_magic(&[0u8; 512]);
        assert_eq!(i.format, DetectedFormat::Raw);
        assert!(i.virtual_size.is_none());
        let i = detect_magic(b"QF");
        assert_eq!(i.format, DetectedFormat::Raw);
        assert!(check_declared(ImageFormat::Qcow2, &i).is_err());
        check_declared(ImageFormat::Raw, &i).expect("raw is raw");
    }
}
