// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The DNS resource model: `Zone` and `RecordSet`.
//!
//! A zone is an authoritative apex (`example.cloud.`) with the SOA and NS
//! data synthesised from its spec; a record set is one owner name and one
//! type inside a zone with one or more RDATA strings. `spec` is the user's,
//! `status` is the reconciler's: it carries the zone serial, which the
//! reconciler bumps whenever the zone's content changes.
//!
//! Names are stored as absolute, lower-case FQDNs with a trailing dot.

use pistack_types::{kind, ApiError, ApiResult, Id, Kind, Meta, Phase, Resource};
use serde::{Deserialize, Serialize};
use std::net::{Ipv4Addr, Ipv6Addr};

/// Define a resource struct with the `Resource<Spec, Status>` wire shape
/// (`meta` flattened, then `spec` and `status`). A named struct rather than
/// the alias because the orphan rule forbids `impl Kind for Resource<..>`
/// outside `pistack-types`; `From` both ways keeps them interchangeable.
macro_rules! resource {
    ($(#[$doc:meta])* $t:ident, $spec:ty, $status:ty, $k:expr) => {
        $(#[$doc])*
        #[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
        pub struct $t {
            #[serde(flatten)]
            pub meta: Meta,
            pub spec: $spec,
            pub status: $status,
        }
        impl Kind for $t {
            const KIND: &'static str = $k;
            fn meta(&self) -> &Meta {
                &self.meta
            }
            fn meta_mut(&mut self) -> &mut Meta {
                &mut self.meta
            }
        }
        impl From<Resource<$spec, $status>> for $t {
            fn from(r: Resource<$spec, $status>) -> Self {
                Self { meta: r.meta, spec: r.spec, status: r.status }
            }
        }
        impl From<$t> for Resource<$spec, $status> {
            fn from(r: $t) -> Self {
                Self { meta: r.meta, spec: r.spec, status: r.status }
            }
        }
    };
}

/// Label that opts a zone into automatic machine records (`auto.rs`).
pub const AUTO_LABEL: &str = "pistack.io/auto";
/// Label beacon puts on record sets it manages itself; users should not
/// edit those by hand, the next pass would overwrite them.
pub const MANAGED_LABEL: &str = "pistack.io/managed-by";
/// How many `also_notify` addresses one primary zone may name. A zone that
/// needs more is a anycast set, not a zone with secondaries.
pub const MAX_ALSO_NOTIFY: usize = 16;

// ---- Zone -------------------------------------------------------------------

/// Primary zones are edited here and served by beacon (including AXFR over
/// TCP). Secondary zones list `primaries` and are filled by AXFR pull: they
/// stay Pending until a transfer succeeds, then Ready and served from the
/// secondary cache (record sets are still primary-only).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ZoneKind {
    #[default]
    Primary,
    Secondary {
        /// Addresses (`ip` or `ip:port`) of the primaries to transfer from.
        primaries: Vec<String>,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ZoneSpec {
    /// Apex name, absolute with a trailing dot: `example.cloud.`.
    pub name: String,
    #[serde(default)]
    pub kind: ZoneKind,
    /// Default TTL for records that do not set one, and the SOA minimum.
    #[serde(default = "default_ttl")]
    pub ttl: u32,
    /// Hostmaster address, written into the SOA RNAME in mailbox form.
    #[serde(default = "default_email")]
    pub email: String,
    /// Authoritative name servers, absolute names. The first is the SOA
    /// MNAME. Defaults to `ns1.<zone>` when empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub nameservers: Vec<String>,
    /// Addresses (`ip` or `ip:port`) of servers outside this cloud that are
    /// secondaries of a primary zone, and are therefore told when its serial
    /// moves. A secondary zone does not take this field.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub also_notify: Vec<String>,
}

pub fn default_ttl() -> u32 {
    3600
}

fn default_email() -> String {
    "hostmaster".into()
}

impl Default for ZoneSpec {
    fn default() -> Self {
        Self {
            name: String::new(),
            kind: ZoneKind::Primary,
            ttl: default_ttl(),
            email: default_email(),
            nameservers: Vec::new(),
            also_notify: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ZoneStatus {
    #[serde(default)]
    pub phase: Phase,
    /// SOA serial. Bumped by the reconciler each time the zone's content
    /// (spec or any record set) changes. Zero until the first pass.
    #[serde(default)]
    pub serial: u32,
    /// Number of record sets in the zone at the last pass.
    #[serde(default)]
    pub records: u32,
    /// Fingerprint of the content the serial was issued for, so a repeated
    /// pass over unchanged data does not bump the serial again.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub digest: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub message: String,
}

resource!(
    /// An authoritative DNS zone.
    Zone,
    ZoneSpec,
    ZoneStatus,
    kind::ZONE
);

impl Zone {
    /// SOA MNAME and NS owner names: the spec's list, or `ns1.<zone>`.
    pub fn nameservers(&self) -> Vec<String> {
        if self.spec.nameservers.is_empty() {
            vec![format!("ns1.{}", self.spec.name)]
        } else {
            self.spec.nameservers.clone()
        }
    }

    /// SOA RNAME: `hostmaster@example.cloud` becomes `hostmaster.example.cloud.`;
    /// a bare local part is placed in the zone itself.
    pub fn soa_rname(&self) -> String {
        let (local, domain) = match self.spec.email.split_once('@') {
            Some((l, d)) => (l.to_owned(), format!("{}.", d.trim_end_matches('.'))),
            None => (self.spec.email.clone(), self.spec.name.clone()),
        };
        format!("{}.{}", local.replace('.', "\\."), domain)
    }
}

// ---- RecordSet ---------------------------------------------------------------

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[serde(rename_all = "UPPERCASE")]
pub enum RecordType {
    A,
    Aaaa,
    Cname,
    Mx,
    Txt,
    Srv,
    Ns,
    Ptr,
}

impl RecordType {
    pub fn as_str(self) -> &'static str {
        match self {
            RecordType::A => "A",
            RecordType::Aaaa => "AAAA",
            RecordType::Cname => "CNAME",
            RecordType::Mx => "MX",
            RecordType::Txt => "TXT",
            RecordType::Srv => "SRV",
            RecordType::Ns => "NS",
            RecordType::Ptr => "PTR",
        }
    }
}

impl std::fmt::Display for RecordType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct RecordSetSpec {
    /// Zone the set belongs to.
    pub zone: Id,
    /// Owner name, absolute (`web.example.cloud.`) or relative to the zone
    /// (`web`, or `@` for the apex). Stored absolute and lower-case.
    pub name: String,
    #[serde(rename = "type")]
    pub kind: RecordType,
    /// Falls back to the zone TTL when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttl: Option<u32>,
    /// RDATA in presentation format, one per record:
    /// `10.0.0.4`, `10 mail.example.cloud.`, `"v=spf1 -all"`.
    pub records: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct RecordSetStatus {
    #[serde(default)]
    pub phase: Phase,
    /// Zone serial the set was first served under.
    #[serde(default)]
    pub serial: u32,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub message: String,
}

resource!(
    /// One owner name and one type in a zone.
    RecordSet,
    RecordSetSpec,
    RecordSetStatus,
    kind::RECORD
);

/// Every kind beacon owns, for the reconciler's event filter.
pub const KINDS: &[&str] = &[kind::ZONE, kind::RECORD];

// ---- validation ---------------------------------------------------------------

/// Normalise a domain name: lower-case, trailing dot, each label 1-63
/// LDH characters (underscore allowed, for `_acme-challenge` and SRV
/// owners; a leading `*` label for wildcards). 253 characters at most.
pub fn normalize_name(raw: &str) -> ApiResult<String> {
    let s = raw.trim().to_ascii_lowercase();
    if s.is_empty() {
        return Err(ApiError::bad_request("name must not be empty"));
    }
    if s == "." {
        return Err(ApiError::bad_request("the root zone cannot be managed"));
    }
    let body = s.trim_end_matches('.');
    if body.len() > 253 {
        return Err(ApiError::bad_request(format!(
            "name {raw} is longer than 253 characters"
        )));
    }
    let labels: Vec<&str> = body.split('.').collect();
    for (i, label) in labels.iter().enumerate() {
        if label.is_empty() {
            return Err(ApiError::bad_request(format!(
                "name {raw} has an empty label"
            )));
        }
        if label.len() > 63 {
            return Err(ApiError::bad_request(format!(
                "label {label} in {raw} is longer than 63 characters"
            )));
        }
        if *label == "*" && i == 0 {
            continue;
        }
        let ok = label
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
        if !ok || label.starts_with('-') || label.ends_with('-') {
            return Err(ApiError::bad_request(format!(
                "label {label} in {raw} is not a valid hostname label"
            )));
        }
    }
    Ok(format!("{body}."))
}

/// Resolve a record owner relative to the zone and check it falls inside it.
/// `@` and the empty string mean the apex.
pub fn owner_in_zone(zone: &str, raw: &str) -> ApiResult<String> {
    let raw = raw.trim();
    if raw.is_empty() || raw == "@" {
        return Ok(zone.to_owned());
    }
    let name = if raw.ends_with('.') {
        normalize_name(raw)?
    } else {
        normalize_name(&format!("{raw}.{zone}"))?
    };
    if !in_zone(zone, &name) {
        return Err(ApiError::bad_request(format!(
            "{name} is not inside zone {zone}"
        )));
    }
    Ok(name)
}

/// True when `name` is `zone` or ends in `.<zone>`. Both absolute.
pub fn in_zone(zone: &str, name: &str) -> bool {
    name == zone || name.ends_with(&format!(".{zone}"))
}

pub fn validate_zone(spec: &mut ZoneSpec) -> ApiResult<()> {
    spec.name = normalize_name(&spec.name)?;
    if spec.ttl == 0 {
        return Err(ApiError::bad_request("zone ttl must be greater than zero"));
    }
    if spec.email.trim().is_empty() {
        return Err(ApiError::bad_request("zone email must not be empty"));
    }
    spec.email = spec.email.trim().to_owned();
    if let Some((local, domain)) = spec.email.split_once('@') {
        if local.is_empty() {
            return Err(ApiError::bad_request("zone email has no local part"));
        }
        normalize_name(domain)?;
    }
    let mut ns = Vec::with_capacity(spec.nameservers.len());
    for n in &spec.nameservers {
        let n = normalize_name(n)?;
        if !ns.contains(&n) {
            ns.push(n);
        }
    }
    spec.nameservers = ns;
    let mut also = Vec::with_capacity(spec.also_notify.len());
    for a in &spec.also_notify {
        let a = notify_addr(a)
            .map_err(|e| ApiError::bad_request(format!("also_notify {}", e.message)))?;
        if !also.contains(&a) {
            also.push(a);
        }
    }
    if also.len() > MAX_ALSO_NOTIFY {
        return Err(ApiError::bad_request(format!(
            "a zone names at most {MAX_ALSO_NOTIFY} also_notify addresses"
        )));
    }
    spec.also_notify = also;
    if let ZoneKind::Secondary { primaries } = &spec.kind {
        if !spec.also_notify.is_empty() {
            return Err(ApiError::bad_request(
                "only a primary zone takes also_notify",
            ));
        }
        if primaries.is_empty() {
            return Err(ApiError::bad_request(
                "a secondary zone needs at least one primary",
            ));
        }
        for p in primaries {
            notify_addr(p)?;
        }
    }
    Ok(())
}

/// Check one address written `ip` or `ip:port`, and return it in the form
/// [`crate::notify::parse_addr`] reads: trimmed, a v6 host bracketed only when a
/// port follows. A bare address names the standard port, so a v6 address with
/// several colons and no bracket is the address itself, never a malformed
/// `ip:port` pair. An address whose octets or port do not parse is refused here,
/// where the caller can answer, rather than deep in a notify pass.
fn notify_addr(raw: &str) -> ApiResult<String> {
    let a = raw.trim();
    let bad = |why: &str| ApiError::bad_request(format!("{raw} {why}"));
    // Split into host and port on the last colon. A bracketed host always ends
    // before its port; otherwise only a single colon separates the two, so a
    // v6 address written bare — several colons, no bracket — is all host.
    let bracketed = a.starts_with('[');
    let split = match a.rfind(':') {
        Some(i) if bracketed => Some(i),
        Some(i) if a.matches(':').count() == 1 => Some(i),
        Some(_) => None,
        None => None,
    };
    let (host, port) = match split {
        Some(i) => {
            let (h, p) = (&a[..i], &a[i + 1..]);
            if p.parse::<u16>().is_err() {
                return Err(bad("has a port that is not a number"));
            }
            (h, Some(p))
        }
        None => (a, None),
    };
    let host = host.trim_start_matches('[').trim_end_matches(']');
    if host.parse::<std::net::IpAddr>().is_err() {
        return Err(bad("is not an ip address"));
    }
    Ok(match port {
        Some(p) if host.contains(':') => format!("[{host}]:{p}"),
        Some(p) => format!("{host}:{p}"),
        None => host.to_owned(),
    })
}

/// Normalise and check a record set against its zone. Owner is made
/// absolute, RDATA is syntax-checked per type and canonicalised where it is
/// a name. Does not check CNAME exclusivity against other sets; see
/// `Client`.
pub fn validate_recordset(zone: &Zone, spec: &mut RecordSetSpec) -> ApiResult<()> {
    spec.name = owner_in_zone(&zone.spec.name, &spec.name)?;
    if spec.records.is_empty() {
        return Err(ApiError::bad_request(
            "a record set needs at least one record",
        ));
    }
    if spec.ttl == Some(0) {
        return Err(ApiError::bad_request(
            "record ttl must be greater than zero",
        ));
    }
    let apex = spec.name == zone.spec.name;
    match spec.kind {
        RecordType::Cname if apex => {
            return Err(ApiError::bad_request("a CNAME cannot sit at the zone apex"));
        }
        RecordType::Cname if spec.records.len() > 1 => {
            return Err(ApiError::bad_request(
                "a CNAME set holds exactly one record",
            ));
        }
        RecordType::Ns if apex => {
            return Err(ApiError::bad_request(
                "apex NS records come from the zone's nameservers",
            ));
        }
        _ => {}
    }
    let mut out = Vec::with_capacity(spec.records.len());
    for r in &spec.records {
        let canon = validate_rdata(spec.kind, r)?;
        if out.contains(&canon) {
            return Err(ApiError::bad_request(format!("duplicate record {r}")));
        }
        out.push(canon);
    }
    spec.records = out;
    Ok(())
}

/// Check one RDATA string for `kind`; returns its canonical form.
pub fn validate_rdata(kind: RecordType, raw: &str) -> ApiResult<String> {
    let s = raw.trim();
    if s.is_empty() {
        return Err(ApiError::bad_request("record data must not be empty"));
    }
    let bad = |m: String| ApiError::bad_request(format!("{kind} record {raw:?}: {m}"));
    match kind {
        RecordType::A => s
            .parse::<Ipv4Addr>()
            .map(|a| a.to_string())
            .map_err(|_| bad("not an IPv4 address".into())),
        RecordType::Aaaa => s
            .parse::<Ipv6Addr>()
            .map(|a| a.to_string())
            .map_err(|_| bad("not an IPv6 address".into())),
        RecordType::Cname | RecordType::Ns | RecordType::Ptr => {
            if !s.ends_with('.') {
                return Err(bad(
                    "target must be an absolute name with a trailing dot".into()
                ));
            }
            normalize_name(s).map_err(|e| bad(e.message))
        }
        RecordType::Mx => {
            let (pref, host) = s
                .split_once(char::is_whitespace)
                .ok_or_else(|| bad("expected '<preference> <exchange.>'".into()))?;
            let pref: u16 = pref
                .parse()
                .map_err(|_| bad("preference must be 0-65535".into()))?;
            let host = host.trim();
            if !host.ends_with('.') {
                return Err(bad("exchange must be an absolute name".into()));
            }
            let host = normalize_name(host).map_err(|e| bad(e.message))?;
            Ok(format!("{pref} {host}"))
        }
        RecordType::Srv => {
            let parts: Vec<&str> = s.split_whitespace().collect();
            if parts.len() != 4 {
                return Err(bad("expected '<priority> <weight> <port> <target.>'".into()));
            }
            let mut nums = [0u16; 3];
            for (i, p) in parts.iter().take(3).enumerate() {
                nums[i] = p.parse().map_err(|_| bad(format!("{p} must be 0-65535")))?;
            }
            if !parts[3].ends_with('.') {
                return Err(bad("target must be an absolute name".into()));
            }
            let target = normalize_name(parts[3]).map_err(|e| bad(e.message))?;
            Ok(format!("{} {} {} {target}", nums[0], nums[1], nums[2]))
        }
        RecordType::Txt => {
            // Accept bare or quoted text; store quoted so the zone-file
            // export is unambiguous. Each character string is 255 bytes max.
            let text = if s.starts_with('"') && s.ends_with('"') && s.len() >= 2 {
                &s[1..s.len() - 1]
            } else {
                s
            };
            if text.len() > 255 {
                return Err(bad("text longer than 255 bytes".into()));
            }
            Ok(format!("\"{}\"", text.replace('"', "\\\"")))
        }
    }
}

/// Render the zone in master-file format (RFC 1035 §5), SOA and apex NS
/// first, then every set sorted by owner and type.
pub fn export_zone(zone: &Zone, sets: &[RecordSet]) -> String {
    let mut out = String::new();
    let apex = &zone.spec.name;
    out.push_str(&format!("$ORIGIN {apex}\n$TTL {}\n", zone.spec.ttl));
    let ns = zone.nameservers();
    out.push_str(&format!(
        "{apex} {} IN SOA {} {} {} 7200 3600 1209600 {}\n",
        zone.spec.ttl,
        ns[0],
        zone.soa_rname(),
        zone.status.serial,
        zone.spec.ttl
    ));
    for n in &ns {
        out.push_str(&format!("{apex} {} IN NS {n}\n", zone.spec.ttl));
    }
    let mut sorted: Vec<&RecordSet> = sets.iter().collect();
    sorted.sort_by(|a, b| {
        a.spec
            .name
            .cmp(&b.spec.name)
            .then(a.spec.kind.as_str().cmp(b.spec.kind.as_str()))
    });
    for rs in sorted {
        let ttl = rs.spec.ttl.unwrap_or(zone.spec.ttl);
        for r in &rs.spec.records {
            out.push_str(&format!("{} {ttl} IN {} {r}\n", rs.spec.name, rs.spec.kind));
        }
    }
    out
}

#[cfg(test)]
mod tests {
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
            validate_recordset(&z, &mut rs(RecordType::A, "web", &["10.0.0.4", "10.0.0.4"]))
                .is_err()
        );
        assert!(validate_recordset(&z, &mut rs(RecordType::A, "web", &[])).is_err());

        validate_recordset(&z, &mut rs(RecordType::Aaaa, "web", &["2001:db8::1"])).unwrap();
        assert!(validate_recordset(&z, &mut rs(RecordType::Aaaa, "web", &["10.0.0.1"])).is_err());

        let mut c = rs(RecordType::Cname, "www", &["Web.Example.Cloud."]);
        validate_recordset(&z, &mut c).unwrap();
        assert_eq!(c.records, vec!["web.example.cloud."]);
        assert!(validate_recordset(&z, &mut rs(RecordType::Cname, "www", &["web"])).is_err());
        assert!(
            validate_recordset(&z, &mut rs(RecordType::Cname, "@", &["web.example.cloud."]))
                .is_err()
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
        assert!(
            validate_recordset(&z, &mut rs(RecordType::Txt, "@", &[&"x".repeat(256)])).is_err()
        );

        assert!(
            validate_recordset(&z, &mut rs(RecordType::Ns, "@", &["ns1.example.cloud."])).is_err()
        );
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
}
