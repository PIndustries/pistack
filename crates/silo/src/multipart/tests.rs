// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use chrono::Utc;

use super::normalize_etag::normalize_etag;
use super::*;

#[test]
fn parse_complete_parts() {
    let xml = r#"<?xml version="1.0"?>
<CompleteMultipartUpload>
  <Part><PartNumber>1</PartNumber><ETag>"aabb"</ETag></Part>
  <Part><PartNumber>2</PartNumber><ETag>ccdd</ETag></Part>
</CompleteMultipartUpload>"#;
    let parts = parse_complete_xml(xml).unwrap();
    assert_eq!(parts.len(), 2);
    assert_eq!(parts[0].part_number, 1);
    assert_eq!(normalize_etag(&parts[0].etag), "aabb");
    assert_eq!(parts[1].part_number, 2);
}

#[test]
fn parse_delete_objects_keys_and_quiet() {
    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<Delete><Quiet>true</Quiet>
  <Object><Key>a.txt</Key></Object>
  <Object><Key>dir/b &amp; c</Key><Quiet>false</Quiet></Object>
</Delete>"#;
    let got = parse_delete_xml(xml).unwrap();
    assert!(got.quiet);
    assert_eq!(
        got.objects,
        vec![
            DeleteRequest {
                key: "a.txt".into(),
                quiet: None
            },
            DeleteRequest {
                key: "dir/b & c".into(),
                quiet: Some(false)
            },
        ]
    );
}

#[test]
fn parse_delete_objects_takes_no_object_as_malformed() {
    for body in [
        "<Delete></Delete>",
        "<Delete><Quiet>true</Quiet></Delete>",
        "<Delete><Object><Key>a</Key></Delete>",
        "<Delete><Object><Quiet>true</Quiet></Object></Delete>",
        "<Delete><Object><Key>a</Key><Quiet>yes</Quiet></Object></Delete>",
    ] {
        let err = parse_delete_xml(body).unwrap_err();
        assert_eq!(s3_code_of(&err), Some("MalformedXML"), "{body}");
    }
}

#[test]
fn parse_delete_objects_keeps_a_key_that_looks_like_markup() {
    // Only the entities are undone; a body carries a key, not a URL.
    let xml = "<Delete><Object><Key>a%20b&lt;c&gt;</Key></Object></Delete>";
    assert_eq!(parse_delete_xml(xml).unwrap().objects[0].key, "a%20b<c>");
}

#[test]
fn collapse_uploads_via_shared_delimiter() {
    let now = Utc::now();
    let uploads = vec![
        UploadSummary {
            key: "a/".into(),
            upload_id: "u0".into(),
            initiated: now,
        },
        UploadSummary {
            key: "a/1".into(),
            upload_id: "u1".into(),
            initiated: now,
        },
        UploadSummary {
            key: "a/2".into(),
            upload_id: "u2".into(),
            initiated: now,
        },
        UploadSummary {
            key: "b".into(),
            upload_id: "u3".into(),
            initiated: now,
        },
        UploadSummary {
            key: "c/x".into(),
            upload_id: "u4".into(),
            initiated: now,
        },
    ];
    let entries: Vec<_> =
        crate::delimiter::collapse_with_delimiter(uploads.clone(), "", "/", |u| u.key.as_str())
            .into_iter()
            .map(|e| match e {
                crate::delimiter::Delimited::Item(u) => ListUploadEntry::Upload(u),
                crate::delimiter::Delimited::CommonPrefix(cp) => ListUploadEntry::CommonPrefix(cp),
            })
            .collect();
    assert_eq!(
        entries,
        vec![
            ListUploadEntry::CommonPrefix("a/".into()),
            ListUploadEntry::Upload(uploads[3].clone()),
            ListUploadEntry::CommonPrefix("c/".into()),
        ]
    );
}
