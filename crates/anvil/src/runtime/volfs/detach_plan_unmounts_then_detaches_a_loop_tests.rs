// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::volfs::detach_plan::detach_plan;

#[test]
fn detach_plan_unmounts_then_detaches_a_loop() {
    let with_loop = detach_plan("/mnt/vol", Some("/dev/loop7"));
    assert_eq!(
        with_loop,
        vec![
            vec!["umount".to_owned(), "/mnt/vol".to_owned()],
            vec![
                "losetup".to_owned(),
                "-d".to_owned(),
                "/dev/loop7".to_owned()
            ],
        ]
    );
    let device = detach_plan("/mnt/vol", None);
    assert_eq!(
        device,
        vec![vec!["umount".to_owned(), "/mnt/vol".to_owned()]]
    );
}
