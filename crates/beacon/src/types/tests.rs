// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::{kind, Id, Meta};

use super::*;

fn zone(name: &str) -> Zone {
    Zone {
        meta: Meta::new(kind::ZONE, "z", None),
        spec: ZoneSpec {
            name: name.into(),
            ..ZoneSpec::default()
        },
        status: ZoneStatus::default(),
    }
}

fn rs(kind: RecordType, name: &str, recs: &[&str]) -> RecordSetSpec {
    RecordSetSpec {
        zone: Id::new(pistack_types::kind::ZONE),
        name: name.into(),
        kind,
        ttl: None,
        records: recs.iter().map(|s| s.to_string()).collect(),
    }
}

#[test]
fn names_are_normalised() {
    assert_eq!(normalize_name("Example.Cloud").unwrap(), "example.cloud.");
    assert_eq!(normalize_name("example.cloud.").unwrap(), "example.cloud.");
    assert_eq!(
        normalize_name("*.example.cloud").unwrap(),
        "*.example.cloud."
    );
    assert_eq!(
        normalize_name("_acme-challenge.example.cloud").unwrap(),
        "_acme-challenge.example.cloud."
    );
    assert!(normalize_name("").is_err());
    assert!(normalize_name(".").is_err());
    assert!(normalize_name("a..b").is_err());
    assert!(normalize_name("-bad.example.").is_err());
    assert!(normalize_name("bad!.example.").is_err());
    assert!(normalize_name(&"a".repeat(64)).is_err());
}

#[test]
fn owner_must_be_in_zone() {
    let z = "example.cloud.";
    assert_eq!(owner_in_zone(z, "web").unwrap(), "web.example.cloud.");
    assert_eq!(owner_in_zone(z, "@").unwrap(), z);
    assert_eq!(owner_in_zone(z, "").unwrap(), z);
    assert_eq!(
        owner_in_zone(z, "Web.Example.Cloud.").unwrap(),
        "web.example.cloud."
    );
    assert!(owner_in_zone(z, "web.other.cloud.").is_err());
    assert!(owner_in_zone(z, "notexample.cloud.").is_err());
}

#[test]
fn zone_validation() {
    let mut s = ZoneSpec {
        name: "Example.Cloud".into(),
        email: "ops@example.cloud".into(),
        nameservers: vec!["NS1.example.cloud".into(), "ns1.example.cloud.".into()],
        ..ZoneSpec::default()
    };
    validate_zone(&mut s).unwrap();
    assert_eq!(s.name, "example.cloud.");
    assert_eq!(s.nameservers, vec!["ns1.example.cloud."]);

    let mut z = zone("example.cloud.");
    z.spec.email = "ops@example.cloud".into();
    assert_eq!(z.soa_rname(), "ops.example.cloud.");
    z.spec.email = "hostmaster".into();
    assert_eq!(z.soa_rname(), "hostmaster.example.cloud.");
    z.spec.email = "first.last@example.cloud".into();
    assert_eq!(z.soa_rname(), "first\\.last.example.cloud.");

    let mut bad = ZoneSpec {
        name: "x.".into(),
        ttl: 0,
        ..ZoneSpec::default()
    };
    assert!(validate_zone(&mut bad).is_err());
    let mut sec = ZoneSpec {
        name: "x.".into(),
        kind: ZoneKind::Secondary { primaries: vec![] },
        ..ZoneSpec::default()
    };
    assert!(validate_zone(&mut sec).is_err());
    let mut sec = ZoneSpec {
        name: "x.".into(),
        kind: ZoneKind::Secondary {
            primaries: vec!["10.0.0.1:53".into(), "[::1]:53".into()],
        },
        ..ZoneSpec::default()
    };
    validate_zone(&mut sec).unwrap();
}

#[test]
fn also_notify_addresses_are_validated() {
    let mut s = ZoneSpec {
        name: "x.".into(),
        also_notify: vec![
            " 198.51.100.7 ".into(),
            "203.0.113.9:5353".into(),
            "[2001:db8::1]:5353".into(),
            "2001:db8::2".into(),
            "198.51.100.7".into(),
        ],
        ..ZoneSpec::default()
    };
    validate_zone(&mut s).unwrap();
    assert_eq!(
        s.also_notify,
        vec![
            "198.51.100.7",
            "203.0.113.9:5353",
            "[2001:db8::1]:5353",
            "2001:db8::2"
        ],
        "a repeat of a name address is dropped"
    );

    // The bare form is a v6 address, not a malformed `ip:port`.
    let mut s = ZoneSpec {
        name: "x.".into(),
        also_notify: vec!["2001:db8::1".into()],
        ..ZoneSpec::default()
    };
    validate_zone(&mut s).unwrap();

    for bad in ["ns1.example.net", "198.51.100.7:ssh", "198.51.100.999", ""] {
        let mut s = ZoneSpec {
            name: "x.".into(),
            also_notify: vec![bad.into()],
            ..ZoneSpec::default()
        };
        assert!(validate_zone(&mut s).is_err(), "{bad} was accepted");
    }

    let many: Vec<String> = (0..=MAX_ALSO_NOTIFY)
        .map(|i| format!("198.51.100.{}", i % 256))
        .collect();
    let mut s = ZoneSpec {
        name: "x.".into(),
        also_notify: many.iter().take(MAX_ALSO_NOTIFY).cloned().collect(),
        ..ZoneSpec::default()
    };
    validate_zone(&mut s).unwrap();
    let mut s = ZoneSpec {
        name: "x.".into(),
        also_notify: many.clone(),
        ..ZoneSpec::default()
    };
    assert!(
        validate_zone(&mut s).is_err(),
        "{} addresses were accepted",
        many.len()
    );

    // Only a primary tells anyone about a change.
    let mut s = ZoneSpec {
        name: "x.".into(),
        kind: ZoneKind::Secondary {
            primaries: vec!["10.0.0.1".into()],
        },
        also_notify: vec!["198.51.100.7".into()],
        ..ZoneSpec::default()
    };
    assert!(validate_zone(&mut s).is_err());
}

#[test]
fn rdata_per_type() {
    let z = zone("example.cloud.");
    let mut a = rs(RecordType::A, "web", &["10.0.0.4", " 10.0.0.5 "]);
    validate_recordset(&z, &mut a).unwrap();
    assert_eq!(a.name, "web.example.cloud.");
    assert_eq!(a.records, vec!["10.0.0.4", "10.0.0.5"]);
    assert!(validate_recordset(&z, &mut rs(RecordType::A, "web", &["::1"])).is_err());
    assert!(
        validate_recordset(&z, &mut rs(RecordType::A, "web", &["10.0.0.4", "10.0.0.4"])).is_err()
    );
    assert!(validate_recordset(&z, &mut rs(RecordType::A, "web", &[])).is_err());

    validate_recordset(&z, &mut rs(RecordType::Aaaa, "web", &["2001:db8::1"])).unwrap();
    assert!(validate_recordset(&z, &mut rs(RecordType::Aaaa, "web", &["10.0.0.1"])).is_err());

    let mut c = rs(RecordType::Cname, "www", &["Web.Example.Cloud."]);
    validate_recordset(&z, &mut c).unwrap();
    assert_eq!(c.records, vec!["web.example.cloud."]);
    assert!(validate_recordset(&z, &mut rs(RecordType::Cname, "www", &["web"])).is_err());
    assert!(
        validate_recordset(&z, &mut rs(RecordType::Cname, "@", &["web.example.cloud."])).is_err()
    );
    assert!(validate_recordset(
        &z,
        &mut rs(
            RecordType::Cname,
            "www",
            &["a.example.cloud.", "b.example.cloud."]
        )
    )
    .is_err());

    let mut mx = rs(RecordType::Mx, "@", &["10   Mail.example.cloud."]);
    validate_recordset(&z, &mut mx).unwrap();
    assert_eq!(mx.records, vec!["10 mail.example.cloud."]);
    assert!(
        validate_recordset(&z, &mut rs(RecordType::Mx, "@", &["mail.example.cloud."])).is_err()
    );
    assert!(validate_recordset(
        &z,
        &mut rs(RecordType::Mx, "@", &["70000 mail.example.cloud."])
    )
    .is_err());

    let mut srv = rs(
        RecordType::Srv,
        "_sip._tcp",
        &["10 60 5060 sip.example.cloud."],
    );
    validate_recordset(&z, &mut srv).unwrap();
    assert_eq!(srv.name, "_sip._tcp.example.cloud.");
    assert!(validate_recordset(
        &z,
        &mut rs(RecordType::Srv, "_sip._tcp", &["10 60 sip.example.cloud."])
    )
    .is_err());

    let mut txt = rs(RecordType::Txt, "@", &["v=spf1 -all", "\"quoted\""]);
    validate_recordset(&z, &mut txt).unwrap();
    assert_eq!(txt.records, vec!["\"v=spf1 -all\"", "\"quoted\""]);
    assert!(validate_recordset(&z, &mut rs(RecordType::Txt, "@", &[&"x".repeat(256)])).is_err());

    assert!(validate_recordset(&z, &mut rs(RecordType::Ns, "@", &["ns1.example.cloud."])).is_err());
    validate_recordset(
        &z,
        &mut rs(RecordType::Ns, "sub", &["ns1.sub.example.cloud."]),
    )
    .unwrap();
    validate_recordset(&z, &mut rs(RecordType::Ptr, "4", &["web.example.cloud."])).unwrap();
}

#[test]
fn export_is_master_file() {
    let mut z = zone("example.cloud.");
    z.status.serial = 7;
    let a = RecordSet {
        meta: Meta::new(kind::RECORD, "web-a", None),
        spec: rs(RecordType::A, "web.example.cloud.", &["10.0.0.4"]),
        status: RecordSetStatus::default(),
    };
    let text = export_zone(&z, &[a]);
    assert!(text.starts_with("$ORIGIN example.cloud.\n$TTL 3600\n"));
    assert!(text.contains("IN SOA ns1.example.cloud. hostmaster.example.cloud. 7 "));
    assert!(text.contains("example.cloud. 3600 IN NS ns1.example.cloud.\n"));
    assert!(text.contains("web.example.cloud. 3600 IN A 10.0.0.4\n"));
}
