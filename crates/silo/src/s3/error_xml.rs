// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::xml_escape::xml_escape;

/// The `<Error>` document every S3 route answers with. Also what a follower
/// gives an S3 client it cannot serve, so the shape is not invented twice.
pub fn error_xml(code: &str, message: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><Error><Code>{}</Code><Message>{}</Message></Error>",
        code,
        xml_escape(message)
    )
}
