// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

    #[test]
    pub(super) fn exec_output_trims_a_split_utf8_sequence() {
        assert_eq!(bytes_to_text("é".as_bytes()), "é");
        assert_eq!(bytes_to_text(&[0xc3]), "");
        assert_eq!(bytes_to_text(&[0xc3, 0xa9, 0xc3]), "é");
        assert_eq!(bytes_to_text(b"ok"), "ok");
    }
