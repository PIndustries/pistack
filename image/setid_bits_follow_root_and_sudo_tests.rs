// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

    #[test]
    pub(super) fn setid_bits_follow_root_and_sudo() {
        assert_eq!(mask_mode(0o4755, false), 0o755);
        assert_eq!(mask_mode(0o2755, false), 0o755);
        assert_eq!(mask_mode(0o6755, false), 0o755);
        assert_eq!(mask_mode(0o1755, false), 0o1755);
        assert_eq!(mask_mode(0o4755, true), 0o4755);
        assert_eq!(mask_mode(0o2755, true), 0o2755);
        assert_eq!(mask_mode(0o1777, true), 0o1777);
    }
