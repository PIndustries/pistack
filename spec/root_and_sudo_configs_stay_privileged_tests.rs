// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

    #[test]
    pub(super) fn root_and_sudo_configs_stay_privileged() {
        let limits = Limits {
            vcpus: 2,
            memory_mib: 64,
        };
        let args = vec!["/bin/app".to_string()];
        let env = vec!["PATH=/bin".to_string()];
        for mode in [
            RunMode::privileged(),
            RunMode {
                privilege: Privilege::Sudo {
                    sudo: PathBuf::from("/bin/sudo"),
                },
                cgroup: CgroupCaps {
                    memory: true,
                    cpu: true,
                },
            },
        ] {
            let spec = mode.oci_config(
                Some("host"),
                &limits,
                (&args, &env, "/"),
                &sandbox_namespaces(&mode, None),
                &[],
            );
            assert_eq!(spec["process"]["user"]["uid"], 0);
            assert_eq!(spec["process"]["user"]["gid"], 0);
            assert!(spec["linux"].get("uidMappings").is_none());
            assert!(spec["linux"].get("gidMappings").is_none());
            assert_eq!(
                ns_kinds(&spec),
                vec!["pid", "network", "ipc", "uts", "mount"]
            );
            assert!(spec["linux"]["namespaces"]
                .as_array()
                .unwrap()
                .iter()
                .all(|ns| ns.get("path").is_none()));
            assert_eq!(
                spec["linux"]["resources"]["memory"]["limit"],
                64 * 1024 * 1024
            );
            assert_eq!(spec["linux"]["resources"]["cpu"]["period"], 100_000);
            assert_eq!(spec["linux"]["resources"]["cpu"]["quota"], 200_000);
        }
    }
