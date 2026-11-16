// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::bundles::delete_path;
use crate::runtime::youki::consts::SUDO_ENV;
use crate::runtime::youki::image::keep_setid;
use crate::runtime::youki::image::unpack_image;
use crate::runtime::youki::privilege::Privilege;
use crate::runtime::youki::test_support::assert_layer_modes;
use crate::runtime::youki::test_support::env_lock;
use crate::runtime::youki::test_support::tar_nodes;
use crate::runtime::youki::test_support::EnvSlot;
use crate::runtime::youki::test_support::TreeGuard;
use std::path::PathBuf;

#[tokio::test]
async fn unpack_keeps_layer_file_modes() {
    let _guard = env_lock().lock().await;
    let _euid = EnvSlot::set("PISTACK_YOUKI_TEST_EUID", "1000");
    let _sudo = EnvSlot::clear(SUDO_ENV);
    assert!(!keep_setid());

    let tmp = std::env::temp_dir().join(format!(
        "pistack-youki-modes-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&tmp).unwrap();
    let _dir = TreeGuard(tmp.clone());

    let layer = tar_nodes(&[
        ("rodir", b"", b'5', 0o555, ""),
        ("rodir/inside", b"in", b'0', 0o644, ""),
        ("bin/httpd", b"httpd", b'0', 0o755, ""),
        ("etc/config", b"cfg", b'0', 0o644, ""),
        ("bin/suid", b"s", b'0', 0o4755, ""),
        ("usr/bin/tool", b"tool", b'0', 0o755, ""),
        ("linkbin", b"", b'2', 0o777, "usr/bin"),
        ("linkbin/extra", b"ex", b'0', 0o644, ""),
        ("bin/httpd-hl", b"", b'1', 0o755, "bin/httpd"),
    ]);

    let tar_path = tmp.join("layer.tar");
    std::fs::write(&tar_path, &layer).unwrap();
    let root = tmp.join("root");
    std::fs::create_dir(&root).unwrap();
    unpack_image(&tar_path, &root).unwrap();
    assert_layer_modes(&root);

    let oci = tmp.join("oci");
    let blobs = oci.join("blobs").join("sha256");
    std::fs::create_dir_all(&blobs).unwrap();
    let manifest = "aa".repeat(32);
    let config = "bb".repeat(32);
    let layer_hex = "cc".repeat(32);
    std::fs::write(
        oci.join("oci-layout"),
        b"{\"imageLayoutVersion\":\"1.0.0\"}",
    )
    .unwrap();
    std::fs::write(
        oci.join("index.json"),
        format!(r#"{{"manifests":[{{"digest":"sha256:{manifest}"}}]}}"#),
    )
    .unwrap();
    std::fs::write(
        blobs.join(&manifest),
        format!(
            r#"{{"config":{{"digest":"sha256:{config}"}},"layers":[{{"mediaType":"application/vnd.oci.image.layer.v1.tar","digest":"sha256:{layer_hex}"}}]}}"#
        ),
    )
    .unwrap();
    std::fs::write(
        blobs.join(&config),
        br#"{"config":{"Entrypoint":["/bin/httpd"]}}"#,
    )
    .unwrap();
    std::fs::write(blobs.join(&layer_hex), &layer).unwrap();
    let oci_root = tmp.join("oci-root");
    std::fs::create_dir(&oci_root).unwrap();
    let cfg = unpack_image(&oci, &oci_root).unwrap();
    assert_eq!(cfg.entrypoint, vec!["/bin/httpd".to_string()]);
    assert_layer_modes(&oci_root);

    // A rootless agent owns these directories, including mode 0555, and
    // must be able to remove them without the cleanup helper.
    let rootless = Privilege::Rootless {
        uid: 1000,
        gid: 1000,
    };
    delete_path(&root, &rootless).unwrap();
    assert!(!root.exists());
    delete_path(&oci_root, &rootless).unwrap();
    assert!(!oci_root.exists());

    let outside = PathBuf::from(format!(
        "/tmp/yk-out-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&outside).unwrap();
    let _outside = TreeGuard(outside.clone());
    let link = outside.to_str().unwrap();
    assert!(link.len() < 100, "{link}");
    assert!(link.starts_with('/'));
    assert!(!link.contains(".."));
    let bad = tar_nodes(&[
        ("jump", b"", b'2', 0o777, link),
        ("jump/pwned", b"pwned", b'0', 0o644, ""),
    ]);
    let bad_tar = tmp.join("escape.tar");
    std::fs::write(&bad_tar, bad).unwrap();
    let bad_root = tmp.join("escape-root");
    std::fs::create_dir(&bad_root).unwrap();
    let err = unpack_image(&bad_tar, &bad_root).unwrap_err();
    assert!(err.message.contains("image cannot be unpacked"), "{err}");
    assert!(!outside.join("pwned").exists());
    assert!(!bad_root.join("jump").join("pwned").exists());

    std::env::set_var(SUDO_ENV, "1");
    assert!(keep_setid());
    std::env::remove_var(SUDO_ENV);
    std::env::set_var("PISTACK_YOUKI_TEST_EUID", "0");
    assert!(keep_setid());
}
