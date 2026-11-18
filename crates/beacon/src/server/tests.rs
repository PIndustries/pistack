// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::Client;
use crate::types::{RecordType as Rt, Zone};
use hickory_proto::op::ResponseCode;
use hickory_proto::rr::{Name, RecordType};
use std::net::{IpAddr, SocketAddr};
use std::str::FromStr;

use super::private_ip_from_hint::private_ip_from_hint;
use super::*;
use crate::types::ZoneSpec;
use pistack_store::Store;
use pistack_types::{kind, Meta};

fn setup() -> (Client, Zone) {
    let client = Client::new(Store::in_memory().unwrap());
    let z = client
        .create_zone(
            Meta::new(kind::ZONE, "example", None),
            ZoneSpec {
                name: "example.cloud.".into(),
                ..ZoneSpec::default()
            },
        )
        .unwrap();
    client.reconcile_zone(&z.meta.id).unwrap();
    (client, z)
}

fn n(s: &str) -> Name {
    Name::from_ascii(s).unwrap()
}

#[test]
fn answers_cname_nodata_nxdomain_wildcard() {
    let (client, z) = setup();
    let up = |name: &str, k: Rt, recs: &[&str]| {
        client
            .upsert_record(
                &z.meta.id,
                name,
                k,
                None,
                recs.iter().map(|s| s.to_string()).collect(),
                "test",
            )
            .unwrap();
    };
    up("web", Rt::A, &["10.0.0.4"]);
    up("www", Rt::Cname, &["web.example.cloud."]);
    up("*.apps", Rt::A, &["10.0.0.9"]);
    up("@", Rt::Mx, &["10 mail.example.cloud."]);
    let h = Handler::new(client.clone(), SecondaryCache::new());

    let a = h
        .resolve(&n("web.example.cloud."), RecordType::A)
        .unwrap()
        .unwrap();
    assert_eq!(a.code, ResponseCode::NoError);
    assert_eq!(a.answers.len(), 1);
    assert_eq!(a.authorities.len(), 1); // NS

    let c = h
        .resolve(&n("www.example.cloud."), RecordType::A)
        .unwrap()
        .unwrap();
    assert_eq!(c.answers.len(), 2);
    assert_eq!(c.answers[0].record_type(), RecordType::CNAME);
    assert_eq!(c.answers[1].record_type(), RecordType::A);

    let nd = h
        .resolve(&n("web.example.cloud."), RecordType::AAAA)
        .unwrap()
        .unwrap();
    assert_eq!(nd.code, ResponseCode::NoError);
    assert!(nd.answers.is_empty());
    assert_eq!(nd.authorities[0].record_type(), RecordType::SOA);

    let nx = h
        .resolve(&n("nope.example.cloud."), RecordType::A)
        .unwrap()
        .unwrap();
    assert_eq!(nx.code, ResponseCode::NXDomain);

    let wc = h
        .resolve(&n("x.apps.example.cloud."), RecordType::A)
        .unwrap()
        .unwrap();
    assert_eq!(wc.code, ResponseCode::NoError);
    assert_eq!(&wc.answers[0].name, &n("x.apps.example.cloud."));

    let soa = h
        .resolve(&n("example.cloud."), RecordType::SOA)
        .unwrap()
        .unwrap();
    assert_eq!(soa.answers.len(), 1);
    let mx = h
        .resolve(&n("example.cloud."), RecordType::MX)
        .unwrap()
        .unwrap();
    assert_eq!(mx.answers.len(), 1);

    assert!(h
        .resolve(&n("other.tld."), RecordType::A)
        .unwrap()
        .is_none());

    // cache invalidates on change
    up("new", Rt::A, &["10.0.0.7"]);
    assert_eq!(
        h.resolve(&n("new.example.cloud."), RecordType::A)
            .unwrap()
            .unwrap()
            .code,
        ResponseCode::NXDomain
    );
    h.invalidate();
    assert_eq!(
        h.resolve(&n("new.example.cloud."), RecordType::A)
            .unwrap()
            .unwrap()
            .answers
            .len(),
        1
    );

    // AXFR: SOA … records … SOA, exact apex only.
    let xfr = h.resolve_axfr(&n("example.cloud.")).unwrap().unwrap();
    assert_eq!(xfr.code, ResponseCode::NoError);
    assert_eq!(xfr.answers.first().unwrap().record_type(), RecordType::SOA);
    assert_eq!(xfr.answers.last().unwrap().record_type(), RecordType::SOA);
    assert!(xfr.answers.len() >= 4); // SOA + NS + A(s) + SOA
    assert!(h.resolve_axfr(&n("other.tld.")).unwrap().is_none());
}

#[test]
fn env_set_is_honored_exactly() {
    let one = resolve_listen_addrs(Some("10.0.0.5:5353"), Some("192.168.1.1:8443")).unwrap();
    assert_eq!(one, vec![SocketAddr::from_str("10.0.0.5:5353").unwrap()]);

    let multi =
        resolve_listen_addrs(Some("10.0.0.5:5353, 127.0.0.1:5353"), Some("ignored")).unwrap();
    assert_eq!(
        multi,
        vec![
            SocketAddr::from_str("10.0.0.5:5353").unwrap(),
            SocketAddr::from_str("127.0.0.1:5353").unwrap(),
        ]
    );

    // Explicit all-interfaces is allowed when the operator sets the env.
    let all = resolve_listen_addrs(Some("0.0.0.0:53"), None).unwrap();
    assert_eq!(all, vec![SocketAddr::from_str("0.0.0.0:53").unwrap()]);
}

#[test]
fn unset_with_private_hint_binds_cluster_address() {
    for hint in [
        "10.66.66.1:8443",
        "https://10.66.66.1:8443",
        "http://192.168.1.9:18443/v1",
        "172.16.0.4:1",
    ] {
        let addrs = resolve_listen_addrs(None, Some(hint)).unwrap();
        let ip = private_ip_from_hint(hint).unwrap();
        assert_eq!(
            addrs,
            vec![SocketAddr::new(IpAddr::V4(ip), DEFAULT_PORT)],
            "{hint}"
        );
    }
}

#[test]
fn unset_without_private_falls_back_to_loopback() {
    for hint in [
        None,
        Some(""),
        Some("0.0.0.0:8443"),
        Some("127.0.0.1:8443"),
        Some("8.8.8.8:443"),
        Some("https://api.example.com:8443"),
        Some("::"),
        Some("[::]:8443"),
    ] {
        let addrs = resolve_listen_addrs(None, hint).unwrap();
        assert_eq!(
            addrs,
            vec![SocketAddr::from_str(DEFAULT_LISTEN).unwrap()],
            "{hint:?}"
        );
    }
}

#[test]
fn blank_env_counts_as_unset() {
    let addrs = resolve_listen_addrs(Some("  "), Some("10.1.2.3:9")).unwrap();
    assert_eq!(addrs, vec![SocketAddr::from_str("10.1.2.3:5353").unwrap()]);
}

#[test]
fn bad_env_entry_is_refused() {
    let err = resolve_listen_addrs(Some("not-an-addr"), None).unwrap_err();
    assert!(err.to_string().contains(LISTEN_ENV), "{err}");
    let err = resolve_listen_addrs(Some("10.0.0.1:5353,bogus"), None).unwrap_err();
    assert!(err.to_string().contains("bogus"), "{err}");
}

#[test]
fn defaults_never_use_unspecified() {
    let addrs = resolve_listen_addrs(None, Some("0.0.0.0:8443")).unwrap();
    assert!(!addrs.iter().any(|a| a.ip().is_unspecified()));
}
