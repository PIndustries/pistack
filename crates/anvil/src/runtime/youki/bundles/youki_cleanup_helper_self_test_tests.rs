// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::path::PathBuf;

#[test]
fn youki_cleanup_helper_self_test() {
    // SAFETY: geteuid does not fail and does not touch Rust memory.
    if unsafe { libc::geteuid() } == 0 {
        return;
    }
    let script =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../scripts/pistack-youki-cleanup");
    let output = std::process::Command::new("python3")
        .arg("-c")
        .arg("import runpy, sys; runpy.run_path(sys.argv[1])['self_test']()")
        .arg(&script)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "helper self-test failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
