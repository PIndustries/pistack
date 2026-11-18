// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::deleted_key::DeletedKey;
use super::failed_key::FailedKey;
use super::xml_escape::xml_escape;

pub(crate) fn delete_objects_xml(deleted: &[DeletedKey], failed: &[FailedKey]) -> String {
    let mut body = String::new();
    body.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>");
    body.push_str("<DeleteResult xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">");
    for entry in deleted {
        body.push_str("<Deleted>");
        body.push_str(&format!("<Key>{}</Key>", xml_escape(&entry.key)));
        if entry.marker {
            body.push_str("<DeleteMarker>true</DeleteMarker>");
        }
        body.push_str("</Deleted>");
    }
    for e in failed {
        body.push_str("<Error>");
        body.push_str(&format!("<Key>{}</Key>", xml_escape(&e.key)));
        body.push_str(&format!("<Code>{}</Code>", xml_escape(&e.code)));
        body.push_str(&format!("<Message>{}</Message>", xml_escape(&e.message)));
        body.push_str("</Error>");
    }
    body.push_str("</DeleteResult>");
    body
}
