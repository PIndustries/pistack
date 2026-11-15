// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::rpc_driver::RpcDriver;
use super::test_support::*;
use pistack_rpc::proto::ImageSource;
use pistack_rpc::CommandQueue;
use pistack_store::Store;
use pistack_types::{Id, Meta};

#[test]
fn catalog_ready_file_replaces_image_name() {
    let store = Store::in_memory().unwrap();
    let path = std::env::temp_dir().join(format!(
        "pistack-boot-{}-{}",
        std::process::id(),
        uuid::Uuid::now_v7()
    ));
    std::fs::write(&path, b"disk").unwrap();
    let path_str = path.to_string_lossy().into_owned();
    put_image(&store, "custom-base", "ready", Some(&path_str));
    // Not ready, and a sha256 ref, must not win over the file above.
    put_image(&store, "custom-base-pending", "pending", Some(&path_str));
    put_image(
        &store,
        "hashed",
        "ready",
        Some("sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"),
    );
    put_image_at(
        &store,
        "from-catalog",
        "ready",
        Some("sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"),
        Some(&path_str),
    );

    let driver = RpcDriver::for_node(CommandQueue::default(), "h1").with_store(store);
    let mut m = machine("web");
    m.spec.image = "custom-base".into();
    assert_eq!(driver.request_for(&m).image, path_str);
    m.spec.image = "from-catalog".into();
    assert_eq!(driver.request_for(&m).image, path_str);

    m.spec.image = path.display().to_string();
    assert_eq!(driver.request_for(&m).image, path.display().to_string());
    let _ = std::fs::remove_file(&path);
}
#[test]
fn a_catalog_image_tells_the_agent_where_to_fetch_it() {
    let store = Store::in_memory().unwrap();
    let digest = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    let on_another_server = std::env::temp_dir().join(format!(
        "pistack-boot-elsewhere-{}-{}",
        std::process::id(),
        uuid::Uuid::now_v7()
    ));
    let meta = Meta::new(pistack_types::kind::IMAGE, "noble", None);
    let id = meta.id.to_string();
    store
        .collection::<ImgRow>()
        .create(ImgRow {
            meta,
            spec: serde_json::json!({ "format": "qcow2" }),
            status: serde_json::json!({
                "phase": "ready",
                "sha256": digest,
                "size_bytes": 4096,
                "backing_ref": format!("sha256:{digest}"),
                "file_path": on_another_server.to_string_lossy(),
            }),
        })
        .unwrap();
    // A digest in the blob id alone is enough.
    put_image(&store, "by-ref", "ready", Some(&format!("sha256:{digest}")));
    // Not ready, or no digest: nothing to fetch.
    put_image(
        &store,
        "uploading",
        "creating",
        Some(&format!("sha256:{digest}")),
    );
    put_image(&store, "no-digest", "ready", None);

    let driver = RpcDriver::for_node(CommandQueue::default(), "h1").with_store(store);
    let mut m = machine("web");
    m.spec.image = "noble".into();
    let req = driver.request_for(&m);
    // The file is not on this server, so the name goes out unchanged.
    assert_eq!(req.image, "noble");
    assert_eq!(
        req.image_source,
        Some(ImageSource {
            id,
            sha256: digest.into(),
            size_bytes: Some(4096),
            format: "qcow2".into(),
        })
    );

    m.spec.image = "by-ref".into();
    let source = driver.request_for(&m).image_source.unwrap();
    assert_eq!(source.sha256, digest);

    for name in ["uploading", "no-digest", "no-such-image"] {
        m.spec.image = name.into();
        assert_eq!(driver.request_for(&m).image_source, None, "{name}");
    }
    // A path the user gave is theirs to stage.
    m.spec.image = "/var/lib/images/mine.qcow2".into();
    assert_eq!(driver.request_for(&m).image_source, None);
}

#[test]
fn a_machine_boots_only_an_image_its_project_may_use() {
    let store = Store::in_memory().unwrap();
    let ours = Id::new("proj");
    let theirs = Id::new("proj");
    let file = image_file("visible");

    put_image_for(&store, "private", Some(&theirs), "private", &file);
    put_image_for(&store, "unshared", Some(&theirs), "project", &file);
    let shared = put_image_for(&store, "shared", Some(&theirs), "project", &file);
    put_image_for(&store, "public", Some(&theirs), "public", &file);
    put_image_for(&store, "cloud-private", None, "private", &file);
    put_image_for(&store, "cloud-public", None, "public", &file);
    // A member row on an image that is no longer shared grants nothing.
    let unshared_again = put_image_for(&store, "was-shared", Some(&theirs), "private", &file);
    for image in [&shared, &unshared_again] {
        store
            .collection::<MemberRow>()
            .create(MemberRow {
                meta: Meta::new("imgmem", format!("{image}:{ours}"), Some(theirs.clone())),
                spec: serde_json::json!({ "image": image, "project": ours }),
            })
            .unwrap();
    }

    let driver = RpcDriver::for_node(CommandQueue::default(), "h1").with_store(store);
    let boots = |project: Option<&Id>, image: &str| {
        let req = driver.request_for(&machine_in(project, image));
        if req.image == file {
            true
        } else {
            // Sent as an unknown name, with nothing to download.
            assert_eq!(req.image, image);
            assert_eq!(req.image_source, None);
            false
        }
    };
    for (image, usable) in [
        ("private", false),
        ("unshared", false),
        ("shared", true),
        ("public", true),
        ("cloud-private", false),
        ("cloud-public", true),
        ("was-shared", false),
    ] {
        assert_eq!(boots(Some(&ours), image), usable, "{image}");
        // The owner, and a machine with no project, may use them all.
        assert!(boots(None, image), "{image}");
    }
    for image in ["private", "unshared", "shared", "public", "was-shared"] {
        assert!(boots(Some(&theirs), image), "{image}");
    }
    // A third project is not the member.
    assert!(!boots(Some(&Id::new("proj")), "shared"));
    let _ = std::fs::remove_file(&file);
}

#[test]
fn a_projects_own_image_wins_over_another_of_the_same_name() {
    let store = Store::in_memory().unwrap();
    let ours = Id::new("proj");
    let theirs = Id::new("proj");
    let our_file = image_file("ours");
    let their_file = image_file("theirs");
    let public_file = image_file("public");
    let digest = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    put_image_for(&store, "base", Some(&theirs), "private", &their_file);
    put_image_for(&store, "base", None, "public", &public_file);
    put_image_for(&store, "base", Some(&ours), "private", &our_file);
    // Ours has only a digest here; the public one has its file.
    put_image_for(&store, "remote", None, "public", &public_file);
    let meta = Meta::new(pistack_types::kind::IMAGE, "remote", Some(ours.clone()));
    let remote = meta.id.to_string();
    store
        .collection::<ImgRow>()
        .create(ImgRow {
            meta,
            spec: serde_json::json!({ "format": "qcow2" }),
            status: serde_json::json!({ "phase": "ready", "sha256": digest }),
        })
        .unwrap();

    let driver = RpcDriver::for_node(CommandQueue::default(), "h1").with_store(store);
    let image = |project: &Id, name: &str| driver.request_for(&machine_in(Some(project), name));
    assert_eq!(image(&ours, "base").image, our_file);
    assert_eq!(image(&theirs, "base").image, their_file);
    assert_eq!(image(&Id::new("proj"), "base").image, public_file);
    let req = image(&ours, "remote");
    assert_eq!(req.image, "remote");
    assert_eq!(req.image_source.unwrap().id, remote);
    for file in [our_file, their_file, public_file] {
        let _ = std::fs::remove_file(file);
    }
}
#[test]
fn missing_catalog_image_keeps_the_name() {
    let store = Store::in_memory().unwrap();
    let absent = std::env::temp_dir().join(format!(
        "pistack-boot-missing-{}-{}",
        std::process::id(),
        uuid::Uuid::now_v7()
    ));
    let absent_str = absent.to_string_lossy().into_owned();
    put_image(&store, "gone", "ready", Some(&absent_str));
    put_image(&store, "ubuntu-24.04", "error", None);

    let driver = RpcDriver::for_node(CommandQueue::default(), "h1").with_store(store);
    let mut m = machine("web");
    m.spec.image = "ubuntu-24.04".into();
    assert_eq!(driver.request_for(&m).image, "ubuntu-24.04");
    m.spec.image = "no-such-image".into();
    assert_eq!(driver.request_for(&m).image, "no-such-image");
    m.spec.image = "gone".into();
    assert_eq!(driver.request_for(&m).image, "gone");

    let bare = RpcDriver::for_node(CommandQueue::default(), "h1");
    m.spec.image = "ubuntu-24.04".into();
    assert_eq!(bare.request_for(&m).image, "ubuntu-24.04");
}
