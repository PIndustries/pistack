// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::error::ErrorCode;

use crate::runtime::youki::consts::PAUSE_ELF;
use crate::runtime::youki::volumes::check_device_name;
use crate::runtime::youki::volumes::host_path_ok;
use crate::runtime::youki::volumes::next_device;
use crate::runtime::youki::volumes::AttachedDisk;

#[test]
fn device_names_and_host_paths() {
    assert!(host_path_ok("/var/lib/pistack/vol").is_ok());
    assert_eq!(host_path_ok("rel").unwrap_err().code, ErrorCode::BadRequest);
    assert!(host_path_ok("").is_err());
    assert!(host_path_ok("/tmp/../etc").is_err());
    assert!(host_path_ok("/ok\0").is_err());
    assert!(check_device_name("vdb").is_ok());
    assert!(check_device_name("vdz").is_ok());
    assert!(check_device_name("vda").is_err());
    assert!(check_device_name("vd").is_err());
    assert!(check_device_name("/dev/vdb").is_err());
    let mut disks = Vec::new();
    for c in b'b'..=b'z' {
        let name = next_device(&disks).unwrap();
        assert_eq!(name, format!("vd{}", c as char));
        disks.push(AttachedDisk {
            volume: format!("vol-{c}"),
            path: "/dev/null".into(),
            device: name,
            read_only: false,
            rbd_mapped: false,
        });
    }
    assert_eq!(
        next_device(&disks).unwrap_err().code,
        ErrorCode::Unprocessable
    );
    assert_eq!(PAUSE_ELF.len(), 154);
    assert_eq!(&PAUSE_ELF[..4], b"\x7fELF");
    assert_eq!(&PAUSE_ELF[24..32], &0x400078u64.to_le_bytes());
}
