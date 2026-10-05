// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The authoritative server: a hickory [`RequestHandler`] that answers from
//! the store.
//!
//! Zones are compiled into an in-memory [`Catalog`] on first use and thrown
//! away whenever a `zone` or `rec` event arrives, so an answer is never
//! staler than the last store write. SOA and apex NS records are synthesised
//! from the zone spec and its reconciled serial. UDP and TCP share one port,
//! `PISTACK_DNS_LISTEN` (default `127.0.0.1:5353` so an unset variable does
//! not expose DNS on every interface; set `0.0.0.0:53` in production, where
//! the binary needs `CAP_NET_BIND_SERVICE`).
//!
//! Only queries are served: updates, notifies and zone transfers get NOTIMP.

use crate::client::Client;
use crate::types::{in_zone, RecordType as Rt, Zone, KINDS};
use async_trait::async_trait;
use hickory_proto::op::{Metadata, OpCode, ResponseCode};
use hickory_proto::rr::rdata::{NS, SOA};
use hickory_proto::rr::{LowerName, Name, RData, Record, RecordType};
use hickory_server::net::runtime::Time;
use hickory_server::server::{Request, RequestHandler, ResponseHandler, ResponseInfo};
use hickory_server::zone_handler::MessageResponseBuilder;
use hickory_server::Server;
use parking_lot::RwLock;
use pistack_types::module::CancellationToken;
use pistack_types::Event;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::broadcast;

/// Environment variable naming the listen address.
pub const LISTEN_ENV: &str = "PISTACK_DNS_LISTEN";
/// Unprivileged loopback default when `PISTACK_DNS_LISTEN` is unset.
/// Production should set an explicit address such as `0.0.0.0:53`.
pub const DEFAULT_LISTEN: &str = "127.0.0.1:5353";
/// How long an idle TCP client may hold a connection.
const TCP_TIMEOUT: Duration = Duration::from_secs(10);
/// Longest CNAME chain followed inside one zone.
const MAX_CNAME_DEPTH: usize = 8;

/// One compiled zone: every owner name to its records, SOA and NS included.
struct ZoneData {
    apex: Name,
    soa: Record,
    records: HashMap<Name, Vec<Record>>,
}

/// Every primary zone, compiled.
#[derive(Default)]
pub struct Catalog {
    zones: Vec<ZoneData>,
}

impl Catalog {
    fn build(client: &Client) -> anyhow::Result<Self> {
        let mut zones = Vec::new();
        for z in client.zones().list(None)? {
            if z.spec.kind != crate::types::ZoneKind::Primary {
                continue;
            }
            match compile_zone(client, &z) {
                Ok(d) => zones.push(d),
                Err(e) => tracing::warn!(zone = %z.spec.name, error = %e, "beacon: zone skipped"),
            }
        }
        Ok(Self { zones })
    }

    /// The zone with the longest apex containing `name`.
    fn find(&self, name: &Name) -> Option<&ZoneData> {
        let n = name.to_ascii();
        self.zones
            .iter()
            .filter(|z| in_zone(&z.apex.to_ascii(), &n))
            .max_by_key(|z| z.apex.num_labels())
    }
}

fn compile_zone(client: &Client, z: &Zone) -> anyhow::Result<ZoneData> {
    let apex = Name::from_ascii(&z.spec.name)?;
    let ttl = z.spec.ttl;
    let ns_names = z.nameservers();
    let soa = Record::from_rdata(
        apex.clone(),
        ttl,
        RData::SOA(SOA::new(
            Name::from_ascii(&ns_names[0])?,
            Name::from_ascii(z.soa_rname())?,
            z.status.serial,
            7200,
            3600,
            1_209_600,
            ttl,
        )),
    );
    let mut records: HashMap<Name, Vec<Record>> = HashMap::new();
    let apex_recs = records.entry(apex.clone()).or_default();
    apex_recs.push(soa.clone());
    for n in &ns_names {
        apex_recs.push(Record::from_rdata(
            apex.clone(),
            ttl,
            RData::NS(NS(Name::from_ascii(n)?)),
        ));
    }
    for rs in client.recordsets_in_zone(&z.meta.id)? {
        let owner = Name::from_ascii(&rs.spec.name)?;
        let rtype = proto_type(rs.spec.kind);
        let ttl = rs.spec.ttl.unwrap_or(ttl);
        for r in &rs.spec.records {
            match RData::try_from_str(rtype, r) {
                Ok(rdata) => records
                    .entry(owner.clone())
                    .or_default()
                    .push(Record::from_rdata(owner.clone(), ttl, rdata)),
                Err(e) => tracing::warn!(
                    set = %rs.meta.id, owner = %rs.spec.name, data = %r, error = %e,
                    "beacon: record skipped"
                ),
            }
        }
    }
    Ok(ZoneData { apex, soa, records })
}

fn proto_type(k: Rt) -> RecordType {
    match k {
        Rt::A => RecordType::A,
        Rt::Aaaa => RecordType::AAAA,
        Rt::Cname => RecordType::CNAME,
        Rt::Mx => RecordType::MX,
        Rt::Txt => RecordType::TXT,
        Rt::Srv => RecordType::SRV,
        Rt::Ns => RecordType::NS,
        Rt::Ptr => RecordType::PTR,
    }
}

/// Outcome of a lookup, before it is turned into a message.
#[derive(Debug)]
pub struct Answer {
    pub code: ResponseCode,
    pub answers: Vec<Record>,
    /// NS records on a positive answer, the SOA on NODATA and NXDOMAIN.
    pub authorities: Vec<Record>,
}

impl ZoneData {
    fn lookup(&self, name: &Name, qtype: RecordType) -> Answer {
        let mut answers = Vec::new();
        let mut current = name.clone();
        for _ in 0..MAX_CNAME_DEPTH {
            let recs = match self
                .records
                .get(&current)
                .or_else(|| self.wildcard(&current))
            {
                Some(r) => r,
                None if answers.is_empty() => {
                    return Answer {
                        code: ResponseCode::NXDomain,
                        answers,
                        authorities: vec![self.soa.clone()],
                    };
                }
                // CNAME target not in this zone: the client follows it.
                None => break,
            };
            let cname = recs
                .iter()
                .find(|r| r.record_type() == RecordType::CNAME)
                .cloned();
            match cname {
                Some(c) if qtype != RecordType::CNAME && qtype != RecordType::ANY => {
                    let target = match &c.data {
                        RData::CNAME(t) => t.0.clone(),
                        _ => break,
                    };
                    answers.push(synth_owner(c, &current));
                    if !self.apex.zone_of(&target) {
                        break;
                    }
                    current = target;
                }
                _ => {
                    answers.extend(
                        recs.iter()
                            .filter(|r| qtype == RecordType::ANY || r.record_type() == qtype)
                            .map(|r| synth_owner(r.clone(), &current)),
                    );
                    break;
                }
            }
        }
        if answers.is_empty() {
            // Name exists, type does not: NODATA.
            return Answer {
                code: ResponseCode::NoError,
                answers,
                authorities: vec![self.soa.clone()],
            };
        }
        let authorities = if qtype == RecordType::SOA || qtype == RecordType::NS {
            Vec::new()
        } else {
            self.records
                .get(&self.apex)
                .map(|v| {
                    v.iter()
                        .filter(|r| r.record_type() == RecordType::NS)
                        .cloned()
                        .collect()
                })
                .unwrap_or_default()
        };
        Answer {
            code: ResponseCode::NoError,
            answers,
            authorities,
        }
    }

    /// `*.<parent>` for the closest ancestor inside the zone that has one.
    fn wildcard(&self, name: &Name) -> Option<&Vec<Record>> {
        let mut parent = name.base_name();
        while self.apex.zone_of(&parent) && parent.num_labels() >= self.apex.num_labels() {
            let star = Name::from_ascii("*").ok()?.append_domain(&parent).ok()?;
            if let Some(r) = self.records.get(&star) {
                return Some(r);
            }
            if parent == self.apex {
                break;
            }
            parent = parent.base_name();
        }
        None
    }
}

/// A wildcard match is returned under the queried name, not `*.zone`.
fn synth_owner(mut r: Record, owner: &Name) -> Record {
    if r.name.is_wildcard() {
        r.name = owner.clone();
    }
    r
}

/// Store-backed [`RequestHandler`] with an invalidating cache.
#[derive(Clone)]
pub struct Handler {
    client: Client,
    cache: Arc<RwLock<Option<Arc<Catalog>>>>,
}

impl Handler {
    pub fn new(client: Client) -> Self {
        Self {
            client,
            cache: Arc::new(RwLock::new(None)),
        }
    }

    /// Drop the compiled catalog; the next query rebuilds it.
    pub fn invalidate(&self) {
        *self.cache.write() = None;
    }

    fn catalog(&self) -> anyhow::Result<Arc<Catalog>> {
        if let Some(c) = self.cache.read().as_ref() {
            return Ok(c.clone());
        }
        let built = Arc::new(Catalog::build(&self.client)?);
        *self.cache.write() = Some(built.clone());
        Ok(built)
    }

    /// Watch the event bus and invalidate on every zone or record change.
    pub fn watch(&self, mut rx: broadcast::Receiver<Event>, shutdown: CancellationToken) {
        let me = self.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = shutdown.cancelled() => break,
                    ev = rx.recv() => match ev {
                        Ok(ev) if KINDS.contains(&ev.kind.as_str()) => me.invalidate(),
                        Ok(_) => {}
                        Err(broadcast::error::RecvError::Lagged(_)) => me.invalidate(),
                        Err(broadcast::error::RecvError::Closed) => break,
                    },
                }
            }
        });
    }

    /// Answer a single question. Public so tests can drive the handler
    /// without a socket.
    pub fn resolve(&self, name: &Name, qtype: RecordType) -> anyhow::Result<Option<Answer>> {
        let catalog = self.catalog()?;
        Ok(catalog.find(name).map(|z| z.lookup(name, qtype)))
    }
}

#[async_trait]
impl RequestHandler for Handler {
    async fn handle_request<R: ResponseHandler, T: Time>(
        &self,
        request: &Request,
        mut response_handle: R,
    ) -> ResponseInfo {
        let (code, answers, authorities, authoritative) = match self.classify(request) {
            Ok(a) => (a.code, a.answers, a.authorities, true),
            Err(code) => (code, Vec::new(), Vec::new(), false),
        };
        let mut metadata = Metadata::response_from_request(&request.metadata);
        metadata.authoritative = authoritative;
        metadata.recursion_available = false;
        metadata.response_code = code;
        let (soa, auth): (Vec<Record>, Vec<Record>) = authorities
            .into_iter()
            .partition(|r| r.record_type() == RecordType::SOA);
        let edns = request.edns.as_ref().map(|e| {
            let mut out = hickory_proto::op::Edns::new();
            out.set_max_payload(e.max_payload().max(512)).set_version(0);
            out
        });
        let response = MessageResponseBuilder::new(&request.queries, edns.as_ref()).build(
            metadata,
            answers.iter(),
            auth.iter(),
            soa.iter(),
            std::iter::empty(),
        );
        match response_handle.send_response(response).await {
            Ok(info) => info,
            Err(e) => {
                tracing::warn!(error = %e, "beacon: failed to send response");
                let mut m = Metadata::response_from_request(&request.metadata);
                m.response_code = ResponseCode::ServFail;
                ResponseInfo::from(hickory_proto::op::Header {
                    metadata: m,
                    counts: Default::default(),
                })
            }
        }
    }
}

impl Handler {
    /// Decide what to answer, or which error code to return.
    fn classify(&self, request: &Request) -> Result<Answer, ResponseCode> {
        if request.metadata.op_code != OpCode::Query {
            return Err(ResponseCode::NotImp);
        }
        let info = request.request_info().map_err(|_| ResponseCode::FormErr)?;
        let qtype = info.query.query_type();
        if matches!(qtype, RecordType::AXFR | RecordType::IXFR) {
            return Err(ResponseCode::NotImp);
        }
        let lower: &LowerName = info.query.name();
        let mut name = Name::from(lower);
        name.set_fqdn(true);
        match self.resolve(&name, qtype) {
            Ok(Some(a)) => Ok(a),
            Ok(None) => Err(ResponseCode::Refused),
            Err(e) => {
                tracing::error!(error = %e, "beacon: lookup failed");
                Err(ResponseCode::ServFail)
            }
        }
    }
}

/// The listen address from the environment, or the development default.
pub fn listen_addr() -> anyhow::Result<SocketAddr> {
    let s = std::env::var(LISTEN_ENV).unwrap_or_else(|_| DEFAULT_LISTEN.into());
    SocketAddr::from_str(&s).map_err(|e| anyhow::anyhow!("{LISTEN_ENV}={s}: {e}"))
}

/// Bind UDP and TCP on `listen` and serve until `shutdown` fires. Returns the
/// bound address (useful when `listen` has port 0). The handler is
/// invalidated on every zone/record event from `events`.
pub async fn serve(
    client: Client,
    listen: SocketAddr,
    events: broadcast::Sender<Event>,
    shutdown: CancellationToken,
) -> anyhow::Result<SocketAddr> {
    let udp = tokio::net::UdpSocket::bind(listen).await?;
    let bound = udp.local_addr()?;
    let tcp = tokio::net::TcpListener::bind(bound).await?;

    let handler = Handler::new(client);
    handler.watch(events.subscribe(), shutdown.clone());

    let mut server = Server::new(handler);
    server.register_socket(udp);
    server.register_listener(tcp, TCP_TIMEOUT, 4096);
    tracing::info!(addr = %bound, "beacon: authoritative dns listening (udp+tcp)");

    tokio::spawn(async move {
        shutdown.cancelled().await;
        if let Err(e) = server.shutdown_gracefully().await {
            tracing::warn!(error = %e, "beacon: dns server shutdown");
        }
        tracing::debug!("beacon dns server stopped");
    });
    Ok(bound)
}

#[cfg(test)]
mod tests {
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
        let h = Handler::new(client.clone());

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
    }
}
