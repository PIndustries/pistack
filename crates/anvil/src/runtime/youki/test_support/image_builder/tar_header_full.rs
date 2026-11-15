// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::put_octal;

pub(crate) fn tar_header_full(name: &str, size: u64, flag: u8, mode: u32, link: &str) -> [u8; 512] {
    let mut header = [0u8; 512];
    assert!(name.len() < 100, "{name}");
    assert!(link.len() < 100, "{link}");
    header[..name.len()].copy_from_slice(name.as_bytes());
    if !link.is_empty() {
        header[157..157 + link.len()].copy_from_slice(link.as_bytes());
    }
    put_octal(&mut header[100..108], u64::from(mode));
    put_octal(&mut header[108..116], 0);
    put_octal(&mut header[116..124], 0);
    put_octal(&mut header[124..136], size);
    put_octal(&mut header[136..148], 0);
    header[148..156].fill(b' ');
    header[156] = flag;
    header[257..262].copy_from_slice(b"ustar");
    header[263] = b'0';
    header[264] = b'0';
    let sum: u64 = header.iter().map(|b| u64::from(*b)).sum();
    let text = format!("{sum:06o}");
    header[148..154].copy_from_slice(text.as_bytes());
    header[154] = 0;
    header[155] = b' ';
    header
}
