// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::*;

#[test]
fn an_old_record_reads_as_no_delete_marker() {
    // Records written before this field existed must keep reading.
    let old = r#"{"bucket":"bkt-1","key":"k","sha256":"aa","md5":"bb","size_bytes":2,"version":3,"latest":false}"#;
    let status: ObjectStatus = serde_json::from_str(old).unwrap();
    assert!(!status.delete_marker);
    assert_eq!(status.version, 3);
    assert!(!status.latest);
    // A record with no version at all is still generation 1 and current.
    let bare: ObjectStatus = serde_json::from_str("{}").unwrap();
    assert_eq!(bare.version, 1);
    assert!(bare.latest);
    assert!(!bare.delete_marker);
    let marker = ObjectStatus {
        delete_marker: true,
        ..Default::default()
    };
    assert!(serde_json::to_string(&marker)
        .unwrap()
        .contains("\"delete_marker\":true"));
}

#[test]
fn key_rules() {
    assert!(validate_key("hello.txt").is_ok());
    assert!(validate_key("dir/nested.txt").is_ok());
    assert!(validate_key("").is_err());
    assert!(validate_key("..").is_err());
    assert!(validate_key("../x").is_err());
    assert!(validate_key("a/../../b").is_err());
    assert!(validate_key("/etc/passwd").is_err());
    assert!(validate_key("a//b").is_err());
    assert!(validate_key(".blobs/abcd").is_err());
    assert!(validate_key(".uploads/x").is_err());
    assert!(validate_key(r"foo\bar").is_err());
    assert!(validate_key("a/").is_err());
    // validate_key still allows control chars (legacy objects readable).
    assert!(validate_key("a\u{1}b").is_ok());
    assert!(validate_key("a\tb").is_ok());
}

#[test]
fn xml_unsafe_key_chars() {
    assert!(reject_xml_unsafe_key("hello").is_ok());
    assert!(reject_xml_unsafe_key("a b").is_ok());
    assert!(reject_xml_unsafe_key("a+b").is_ok());
    assert!(reject_xml_unsafe_key("café").is_ok());
    assert!(reject_xml_unsafe_key("a\u{1}b").is_err());
    assert!(reject_xml_unsafe_key("a\tb").is_err());
    assert!(reject_xml_unsafe_key("a\nb").is_err());
    assert!(reject_xml_unsafe_key("a\u{7f}b").is_err());
    assert!(reject_xml_unsafe_key("a\u{fffe}").is_err());
    assert!(reject_xml_unsafe_key("a\u{ffff}").is_err());
    let msg = reject_xml_unsafe_key("a\u{1}b").unwrap_err().to_string();
    assert!(!msg.contains('\u{1}'), "{msg}");
}

#[test]
fn format_limits() {
    assert_eq!(format_byte_limit(MAX_OBJECT_BYTES), "5 GiB");
    assert_eq!(format_byte_limit(64 * 1024), "64 KiB");
    assert_eq!(format_byte_limit(3), "3 bytes");
    assert!(object_too_large_message(1024).contains("1 KiB"));
}

#[test]
fn silo_config_defaults() {
    assert_eq!(SiloConfig::default().max_object_bytes, MAX_OBJECT_BYTES);
    assert_eq!(SiloConfig::default().region, DEFAULT_REGION);
    let cfg =
        pistack_config::Config::from_toml("[modules.silo]\nmax_object_bytes = 1048576\n").unwrap();
    let silo = SiloConfig::from_config(&cfg).unwrap();
    assert_eq!(silo.max_object_bytes, 1024 * 1024);
    assert_eq!(silo.multipart_ttl_secs, MULTIPART_TTL_SECS);
    assert_eq!(silo.region, DEFAULT_REGION);
    let cfg =
        pistack_config::Config::from_toml("[modules.silo]\nregion = \"us-east-1\"\n").unwrap();
    assert_eq!(SiloConfig::from_config(&cfg).unwrap().region, "us-east-1");
}

#[test]
fn silo_config_rejects_empty_region() {
    let cfg = pistack_config::Config::from_toml("[modules.silo]\nregion = \"\"\n").unwrap();
    let err = SiloConfig::from_config(&cfg).unwrap_err();
    assert!(err.to_string().contains("region"), "{err}");
}

#[test]
fn silo_config_multipart_ttl_default_and_override() {
    let default = SiloConfig::default();
    assert_eq!(default.multipart_ttl_secs, 7 * 24 * 60 * 60);
    assert_eq!(
        default.multipart_ttl(),
        std::time::Duration::from_secs(MULTIPART_TTL_SECS)
    );
    assert!(default.validate().is_ok());

    let cfg = pistack_config::Config::from_toml("").unwrap();
    assert_eq!(SiloConfig::from_config(&cfg).unwrap(), default);

    let cfg =
        pistack_config::Config::from_toml("[modules.silo]\nmultipart_ttl_secs = 3600\n").unwrap();
    let silo = SiloConfig::from_config(&cfg).unwrap();
    assert_eq!(silo.multipart_ttl_secs, 3600);
    assert_eq!(silo.max_object_bytes, MAX_OBJECT_BYTES);
}

#[test]
fn silo_config_rejects_zero_multipart_ttl() {
    let cfg =
        pistack_config::Config::from_toml("[modules.silo]\nmultipart_ttl_secs = 0\n").unwrap();
    let err = SiloConfig::from_config(&cfg).unwrap_err();
    assert!(err.to_string().contains("multipart_ttl_secs"), "{err}");
}

#[test]
fn silo_config_rejects_zero() {
    let cfg = pistack_config::Config::from_toml("[modules.silo]\nmax_object_bytes = 0\n").unwrap();
    let err = SiloConfig::from_config(&cfg).unwrap_err();
    assert!(err.to_string().contains("max_object_bytes"), "{err}");
}
