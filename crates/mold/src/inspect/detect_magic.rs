// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::detected_format::DetectedFormat;
use super::inspection::Inspection;
use super::qcow2_magic::QCOW2_MAGIC;

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
