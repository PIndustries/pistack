// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::image::digest_image;
use crate::runtime::youki::image::unpack_image;
use crate::runtime::youki::privilege::RunMode;
use crate::runtime::youki::spec::install_bundle;
use crate::runtime::youki::spec::Limits;
use crate::runtime::youki::test_support::docker_save;
use crate::runtime::youki::test_support::env_lock;
use crate::runtime::youki::test_support::scopeguard_dir;
use crate::runtime::youki::test_support::tar_file;

#[tokio::test]
async fn unpacks_mold_oci_and_rejects_slip_and_unknown_compression() {
    let _guard = env_lock().lock().await;
    let tmp = std::env::temp_dir().join(format!(
        "pistack-youki-unpack-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&tmp).unwrap();
    let _dir = scopeguard_dir(&tmp);
    let prev_mold = std::env::var_os("PISTACK_MOLD_DIR");
    std::env::set_var("PISTACK_MOLD_DIR", &tmp);

    let hex = format!("ab{}", "cd".repeat(31));
    assert_eq!(hex.len(), 64);
    let blob = tmp.join("blobs").join("sha256").join(&hex[..2]).join(&hex);
    std::fs::create_dir_all(blob.parent().unwrap()).unwrap();
    std::fs::write(&blob, tar_file(&[("mold-marker", b"mold")])).unwrap();
    let found = digest_image(&format!("sha256:{hex}")).unwrap();
    assert_eq!(found, blob);
    let mold_root = tmp.join("mold-root");
    std::fs::create_dir(&mold_root).unwrap();
    unpack_image(&found, &mold_root).unwrap();
    assert_eq!(
        std::fs::read_to_string(mold_root.join("mold-marker")).unwrap(),
        "mold"
    );

    let manifest = "aa".repeat(32);
    let config = "bb".repeat(32);
    let layer = "cc".repeat(32);
    let layout = tmp.join("oci");
    let blobs = layout.join("blobs").join("sha256");
    std::fs::create_dir_all(&blobs).unwrap();
    std::fs::write(
        layout.join("oci-layout"),
        b"{\"imageLayoutVersion\":\"1.0.0\"}",
    )
    .unwrap();
    std::fs::write(
        layout.join("index.json"),
        format!(r#"{{"manifests":[{{"digest":"sha256:{manifest}"}}]}}"#),
    )
    .unwrap();
    std::fs::write(
        blobs.join(&manifest),
        format!(
            r#"{{"config":{{"digest":"sha256:{config}"}},"layers":[{{"mediaType":"application/vnd.oci.image.layer.v1.tar","digest":"sha256:{layer}"}}]}}"#
        ),
    )
    .unwrap();
    std::fs::write(
        blobs.join(&config),
        br#"{"config":{"Entrypoint":["/bin/from-oci"],"Cmd":["run"],"Env":["FROM_OCI=1"],"WorkingDir":"/work"}}"#,
    )
    .unwrap();
    std::fs::write(blobs.join(&layer), tar_file(&[("oci-marker", b"oci")])).unwrap();
    let oci_root = tmp.join("oci-root");
    std::fs::create_dir(&oci_root).unwrap();
    let cfg = unpack_image(&layout, &oci_root).unwrap();
    assert_eq!(cfg.entrypoint, vec!["/bin/from-oci".to_string()]);
    assert_eq!(cfg.working_dir, "/work");
    assert_eq!(
        std::fs::read_to_string(oci_root.join("oci-marker")).unwrap(),
        "oci"
    );

    let saved = tmp.join("saved.tar");
    std::fs::write(&saved, docker_save()).unwrap();
    let docker_root = tmp.join("docker-root");
    std::fs::create_dir(&docker_root).unwrap();
    let cfg = unpack_image(&saved, &docker_root).unwrap();
    assert_eq!(cfg.entrypoint, vec!["/bin/app".to_string()]);
    assert_eq!(cfg.cmd, vec!["serve".to_string()]);
    assert_eq!(cfg.working_dir, "/app");
    assert_eq!(
        std::fs::read_to_string(docker_root.join("keep")).unwrap(),
        "yes"
    );
    assert!(!docker_root.join("gone").exists());
    assert!(!docker_root.join("sub").join("old").exists());
    assert_eq!(
        std::fs::read_to_string(docker_root.join("sub").join("new")).unwrap(),
        "n"
    );

    let slip = tmp.join("slip.tar");
    std::fs::write(&slip, tar_file(&[("../escape", b"no")])).unwrap();
    let slip_root = tmp.join("slip-root");
    std::fs::create_dir(&slip_root).unwrap();
    let err = unpack_image(&slip, &slip_root).unwrap_err();
    assert!(err.message.contains("image cannot be unpacked"), "{err}");
    assert!(!tmp.join("escape").exists());
    assert!(!slip_root.join("escape").exists());

    std::fs::write(tmp.join("zstd.bin"), [0x28, 0xb5, 0x2f, 0xfd, 0, 0, 0, 0]).unwrap();
    let err = unpack_image(&tmp.join("zstd.bin"), &slip_root).unwrap_err();
    assert!(
        err.message
            .contains("image layer compression is not available"),
        "{err}"
    );

    std::fs::write(tmp.join("empty-file"), b"").unwrap();
    assert!(unpack_image(&tmp.join("empty-file"), &slip_root).is_err());
    std::os::unix::fs::symlink(&saved, tmp.join("link.tar")).unwrap();
    let err = unpack_image(&tmp.join("link.tar"), &slip_root).unwrap_err();
    assert!(err.message.contains("image cannot be unpacked"), "{err}");

    let src = tmp.join("plain");
    std::fs::create_dir(&src).unwrap();
    std::fs::write(src.join("marker"), "keep").unwrap();
    let outside = tmp.join("outside");
    std::fs::write(&outside, "secret-body").unwrap();
    std::os::unix::fs::symlink(&outside, src.join("abs")).unwrap();
    std::os::unix::fs::symlink("marker", src.join("rel")).unwrap();
    let copied = tmp.join("copied");
    std::fs::create_dir(&copied).unwrap();
    unpack_image(&src, &copied).unwrap();
    assert_eq!(
        std::fs::read_to_string(copied.join("marker")).unwrap(),
        "keep"
    );
    assert!(std::fs::symlink_metadata(copied.join("abs"))
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(std::fs::read_link(copied.join("abs")).unwrap(), outside);
    assert_ne!(
        std::fs::read_to_string(copied.join("marker")).unwrap(),
        "secret-body"
    );

    let empty = tmp.join("empty-dir");
    std::fs::create_dir(&empty).unwrap();
    let err = install_bundle(
        &tmp.join("empty-bundle"),
        &empty,
        "h",
        &Limits {
            vcpus: 1,
            memory_mib: 1,
        },
        None,
        &RunMode::privileged(),
        None,
    )
    .unwrap_err();
    assert!(err.message.contains("image rootfs is empty"), "{err}");

    match prev_mold {
        Some(v) => std::env::set_var("PISTACK_MOLD_DIR", v),
        None => std::env::remove_var("PISTACK_MOLD_DIR"),
    }
}
