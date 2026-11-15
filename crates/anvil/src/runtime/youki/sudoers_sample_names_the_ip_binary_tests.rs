// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::consts::SUDOERS_SAMPLE;

#[test]
pub(super) fn sudoers_sample_names_the_ip_binary() {
    assert!(SUDOERS_SAMPLE.contains("/usr/bin/youki"));
    assert!(SUDOERS_SAMPLE.contains("/usr/local/sbin/pistack-youki-cleanup"));
    assert!(SUDOERS_SAMPLE.contains("/usr/sbin/ip"));
    assert!(SUDOERS_SAMPLE.contains("/usr/bin/ovs-vsctl"));
    assert!(!SUDOERS_SAMPLE.contains("ip link"));
}
