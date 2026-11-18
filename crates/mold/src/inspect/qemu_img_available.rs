// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

/// Is `qemu-img` on `PATH`?
pub fn qemu_img_available() -> bool {
    std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|d| d.join("qemu-img").is_file()))
        .unwrap_or(false)
}
