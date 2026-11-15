// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

    #[test]
    pub(super) fn directory_image_keeps_file_and_directory_modes() {
        let tmp = std::env::temp_dir().join(format!(
            "pistack-youki-dirmode-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&tmp).unwrap();
        let _dir = TreeGuard(tmp.clone());

        let src = tmp.join("src");
        let bin = src.join("bin");
        let ro = src.join("rodir");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::create_dir_all(&ro).unwrap();
        let chmod = |path: &Path, mode: u32| {
            let mut perms = std::fs::symlink_metadata(path).unwrap().permissions();
            perms.set_mode(mode);
            std::fs::set_permissions(path, perms).unwrap();
        };

        let tool = bin.join("tool");
        std::fs::write(&tool, b"#!/bin/sh\n").unwrap();
        chmod(&tool, 0o755);
        let secret = src.join("secret");
        std::fs::write(&secret, b"hidden").unwrap();
        chmod(&secret, 0o600);
        let inside = ro.join("inside");
        std::fs::write(&inside, b"in").unwrap();
        chmod(&inside, 0o644);
        chmod(&bin, 0o750);
        chmod(&ro, 0o555);

        let dst = tmp.join("dst");
        std::fs::create_dir(&dst).unwrap();
        unpack_image(&src, &dst).unwrap();

        assert_eq!(mode_of(&dst.join("bin").join("tool")), 0o755);
        assert_eq!(
            std::fs::read(dst.join("bin").join("tool")).unwrap(),
            b"#!/bin/sh\n"
        );
        assert_eq!(mode_of(&dst.join("secret")), 0o600);
        assert_eq!(std::fs::read(dst.join("secret")).unwrap(), b"hidden");
        assert_eq!(mode_of(&dst.join("bin")), 0o750);
        assert_eq!(mode_of(&dst.join("rodir")), 0o555);
        assert_eq!(mode_of(&dst.join("rodir").join("inside")), 0o644);
        assert_eq!(
            std::fs::read(dst.join("rodir").join("inside")).unwrap(),
            b"in"
        );
    }
