// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::ImageFormat;

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
