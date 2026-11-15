// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Zone change notification (RFC 1996), both directions.
//!
//! Primary side: when a zone's serial moves, every address listed as a
//! primary of a secondary zone for that apex is told over UDP with a
//! `NOTIFY` for `<apex> SOA`, carrying the new serial as the answer hint.
//! Each send is repeated with backoff up to [`ATTEMPTS`] times until the peer
//! answers; the answer is the acknowledgement, and `NOTIMP` counts as one — a
//! peer that will not take notifies will never take them. A zone whose
//! notifies go unanswered keeps its old mark, so a later pass tries again.
//!
//! Secondary side: a NOTIFY is accepted only from an address configured as a
//! primary of a secondary zone this node holds. The zone is then pulled at
//! once instead of at the next refresh. Anything else — an unknown sender, a
//! zone we do not hold, a NOTIFY at a primary — is answered `REFUSED` and
//! pulls nothing.
//!
//! Neither side blocks a caller. Nothing here is on the request path:
//! [`run_notifies`] watches the event bus, gives a burst one window to settle,
//! then acts on what the serials actually say. The notify loop runs wherever the
//! reconciler runs — the leader — and reads secondaries from the same
//! replicated store, so one node notifies for the whole cloud. The receiving
//! side runs on every node that serves DNS, because a NOTIFY can arrive at
//! any of them.

use crate::client::Client;
use crate::server::SecondaryCache;
use crate::types::ZoneKind;
use hickory_proto::op::{
    Header, HeaderCounts, Message, MessageType, Metadata, OpCode, Query, ResponseCode,
};
use hickory_proto::rr::{DNSClass, Name, RData, Record, RecordType};
use hickory_server::server::{Request, ResponseHandler, ResponseInfo};
use hickory_server::zone_handler::MessageResponseBuilder;
use parking_lot::Mutex;
use pistack_types::module::CancellationToken;
use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{broadcast, mpsc};

/// NOTIFY goes to the standard DNS port when a peer listed a bare address;
/// an explicit `ip:port` is used as given.
pub const DNS_PORT: u16 = 53;
/// A zone event waits this long for the rest of its burst before the notify
/// pass runs. Serials move on the reconcile pass, so one window covers one
/// bump however many writes caused it.
pub const COALENSE: Duration = Duration::from_millis(75);
/// A peer that has not answered by now is not acknowledged.
pub const ACK_TIMEOUT: Duration = Duration::from_millis(400);
/// Sends per peer per pass, including the first.
pub const ATTEMPTS: usize = 5;
/// Wait after a missed answer; doubled after every further miss.
pub const BACKOFF: Duration = Duration::from_millis(50);
/// Annotation recording the last serial every peer acknowledged.
pub(crate) const NOTIFIED_KEY: &str = "pistack.io/notified";

/// A `ip` or `ip:port` string as written in a zone spec, with `default_port`
/// filled in for the bare form.
pub fn parse_addr(raw: &str, default_port: u16) -> anyhow::Result<SocketAddr> {
    let s = raw.trim();
    if let Ok(addr) = s.parse::<SocketAddr>() {
        return Ok(addr);
    }
    match s.parse::<IpAddr>() {
        Ok(ip) => Ok(SocketAddr::new(ip, default_port)),
        Err(_) => anyhow::bail!("{raw} is not an address"),
    }
}

/// Apex names are absolute and lower-case in the store, but a NOTIFY arrives
/// from the wire and either side may have been written without the trailing
/// dot.
pub(crate) fn same_apex(a: &str, b: &str) -> bool {
    let t = |s: &str| s.trim_end_matches('.').to_ascii_lowercase();
    t(a) == t(b)
}

/// The addresses a primary zone must notify: every address listed as a
/// primary of a secondary zone for the same apex, plus every address the zone
/// itself names in `also_notify`. The first covers the secondaries held inside
/// this cloud; the second covers the usual outside secondary, an ordinary DNS
/// server that holds nothing in the store and so cannot be discovered. A bare
/// address is taken as the standard DNS port, which is where a peer running on
/// the plain port is. Each address appears once however many places name it.
pub fn peers_for(client: &Client, zone: &crate::types::Zone) -> anyhow::Result<Vec<SocketAddr>> {
    let apex = &zone.spec.name;
    let mut out = Vec::new();
    for raw in &zone.spec.also_notify {
        let addr = parse_addr(raw, DNS_PORT)?;
        if !out.contains(&addr) {
            out.push(addr);
        }
    }
    for z in client.secondary_zones()? {
        if !same_apex(&z.spec.name, apex) {
            continue;
        }
        let ZoneKind::Secondary { primaries } = &z.spec.kind else {
            continue;
        };
        for raw in primaries {
            let addr = parse_addr(raw, DNS_PORT)?;
            if !out.contains(&addr) {
                out.push(addr);
            }
        }
    }
    Ok(out)
}

/// A `NOTIFY` for `<apex> SOA`. The new serial rides in the answer section as
/// the hint RFC 1996 allows; a secondary acts on the AXFR it pulls, not on
/// this number.
pub(crate) fn notify_bytes(id: u16, apex: &str, serial: u32) -> anyhow::Result<Vec<u8>> {
    let name = Name::from_ascii(apex)?;
    let mut message = Message::new(id, MessageType::Query, OpCode::Notify);
    message.metadata.authoritative = true;
    let mut query = Query::new();
    query
        .set_name(name.clone())
        .set_query_class(DNSClass::IN)
        .set_query_type(RecordType::SOA);
    message.add_query(query);
    message.add_answer(Record::from_rdata(
        name.clone(),
        0,
        RData::SOA(hickory_proto::rr::rdata::SOA::new(
            name.clone(),
            name,
            serial,
            0,
            0,
            0,
            0,
        )),
    ));
    Ok(message.to_vec()?)
}

/// The apex a message asks about, absolute and lower-case, as the store keeps
/// zone names.
pub(crate) fn asked_apex(message: &Message) -> Option<String> {
    let mut name = message.queries.first()?.name().clone();
    name.set_fqdn(true);
    Some(name.to_ascii().to_ascii_lowercase())
}

/// Encode a NOTIFY for `apex` at `serial`.
pub fn notify_message(apex: &str, serial: u32) -> anyhow::Result<Vec<u8>> {
    notify_bytes(next_id(), apex, serial)
}

/// May this NOTIFY be acted on? Only when it names a zone this node holds as
/// secondary and comes from a host that zone lists as a primary.
///
/// The port is deliberately not compared. A NOTIFY arrives from an ephemeral
/// source port — nothing sends one from port 53 — so the address a zone lists
/// identifies a machine, and a second tenant on that machine is the same
/// principal as far as the protocol can tell. What is checked is the zone: a
/// sender trusted for one apex gets nothing for another.
pub(crate) fn notify_allowed(
    client: &Client,
    message: &Message,
    src: SocketAddr,
) -> anyhow::Result<bool> {
    if message.metadata.op_code != OpCode::Notify {
        return Ok(false);
    }
    let Some(apex) = asked_apex(message) else {
        return Ok(false);
    };
    Ok(is_listed_primary(&client.secondary_zones()?, &apex, src))
}

/// Does this NOTIFY name a secondary zone we hold, from one of its primaries?
fn is_listed_primary(zones: &[crate::types::Zone], apex: &str, src: SocketAddr) -> bool {
    zones.iter().any(|z| {
        if !same_apex(&z.spec.name, apex) {
            return false;
        }
        let ZoneKind::Secondary { primaries } = &z.spec.kind else {
            return false;
        };
        primaries.iter().any(|raw| match parse_addr(raw, DNS_PORT) {
            Ok(p) => p.ip() == src.ip(),
            Err(_) => false,
        })
    })
}

/// The header of a NOTIFY answer: the request's id, the response bit, the notify
/// opcode kept, AA set, and `code` as the rcode. A NOTIFY answer says nothing
/// beyond that it arrived, so it carries the request's question and no records.
pub(crate) fn notify_answer_header(request: &Metadata, code: ResponseCode) -> Metadata {
    let mut out = Metadata::response_from_request(request);
    out.op_code = OpCode::Notify;
    out.authoritative = true;
    out.response_code = code;
    out
}

/// Answer a NOTIFY that reached the handler with [`notify_answer_header`]. The
/// question is handed over as it arrived and every record section is left
/// empty, so the bytes written to the socket are exactly that header.
pub(crate) async fn answer_notify<R: ResponseHandler>(
    request: &Request,
    code: ResponseCode,
    response_handle: &mut R,
) -> ResponseInfo {
    let mut metadata = notify_answer_header(&request.metadata, code);
    let response = MessageResponseBuilder::new(&request.queries, request.edns.as_ref())
        .build_no_records(metadata);
    match response_handle.send_response(response).await {
        Ok(info) => info,
        Err(e) => {
            tracing::warn!(error = %e, "beacon: notify answer failed");
            metadata.response_code = ResponseCode::ServFail;
            failed(metadata)
        }
    }
}

/// A `ResponseInfo` for a header that never reached the peer.
fn failed(metadata: Metadata) -> ResponseInfo {
    ResponseInfo::from(Header {
        metadata,
        counts: HeaderCounts::default(),
    })
}

/// Send one NOTIFY to `to` and wait for the answer, with backoff. True when
/// the peer answered.
pub(crate) async fn send_notify(
    bytes: &[u8],
    to: SocketAddr,
    acked: &Arc<Mutex<HashSet<(String, SocketAddr)>>>,
    apex: &str,
    shutdown: &CancellationToken,
) -> bool {
    let mut wait = BACKOFF;
    for attempt in 1..=ATTEMPTS {
        if shutdown.is_cancelled() {
            return false;
        }
        // The same address can stand for two zones; if a task for it already
        // heard back, there is nothing left to ask.
        if acked.lock().contains(&(apex.to_owned(), to)) {
            return true;
        }
        match tokio::time::timeout(ACK_TIMEOUT, exchange(bytes, to)).await {
            Ok(Ok(_)) => return true,
            Ok(Err(e)) => tracing::debug!(to = %to, error = %e, "beacon: notify"),
            Err(_) => tracing::debug!(to = %to, attempt, "beacon: notify not acknowledged"),
        }
        if attempt == ATTEMPTS {
            break;
        }
        tokio::select! {
            _ = shutdown.cancelled() => return false,
            _ = tokio::time::sleep(wait) => {}
        }
        wait *= 2;
    }
    false
}

/// Send one NOTIFY for `apex` at `serial` to each address in `peers`, with the
/// same ack and backoff the leader loop uses, and report whether every peer
/// answered. This is what a primary does on a serial change; the leader loop
/// reaches it through [`peers_for`], a caller that already knows its peers
/// reaches it directly.
pub async fn notify_peers(peers: &[String], apex: &str, serial: u32) -> anyhow::Result<()> {
    let bytes = notify_message(apex, serial)?;
    let acked = Arc::new(Mutex::new(HashSet::new()));
    let shutdown = CancellationToken::new();
    let mut tasks = Vec::new();
    for raw in peers {
        let addr = parse_addr(raw, DNS_PORT)?;
        let bytes = bytes.clone();
        let acked = acked.clone();
        let shutdown = shutdown.clone();
        let apex = apex.to_owned();
        tasks.push(tokio::spawn(async move {
            send_notify(&bytes, addr, &acked, &apex, &shutdown).await
        }));
    }
    let mut out = Ok(());
    for task in tasks {
        if !task
            .await
            .map_err(|e| anyhow::anyhow!("notify task: {e}"))?
        {
            out = Err(anyhow::anyhow!("a peer did not acknowledge the NOTIFY"));
        }
    }
    out
}

/// One UDP exchange: send this NOTIFY, await the one answer. The NOTIFY is put
/// on the wire exactly as encoded — its own id, its opcode — so a plain
/// `UdpSocket` is enough and nothing rewrites the message on the way out.
async fn exchange(bytes: &[u8], to: SocketAddr) -> anyhow::Result<ResponseCode> {
    // Bound to the family the peer is in: macOS refuses a v4 destination from
    // an AF_INET6 socket, so one dual-stack socket for both is not available.
    let local = match to.ip() {
        IpAddr::V4(_) => SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0)),
        IpAddr::V6(_) => SocketAddr::from((Ipv6Addr::UNSPECIFIED, 0)),
    };
    let sock = tokio::net::UdpSocket::bind(local).await?;
    let mut buf = [0u8; 4096];
    let id = u16::from_be_bytes([bytes[0], bytes[1]]);
    {
        sock.send_to(bytes, to).await?;
        loop {
            let (n, from) = tokio::time::timeout(ACK_TIMEOUT, sock.recv_from(&mut buf))
                .await
                .map_err(|_| anyhow::anyhow!("{to} did not answer inside {ACK_TIMEOUT:?}"))??;
            if from != to {
                continue;
            }
            let msg = Message::from_vec(&buf[..n])?;
            if msg.id != id {
                continue;
            }
            if msg.metadata.op_code != OpCode::Notify {
                anyhow::bail!("{to} answered a {:?}", msg.metadata.op_code);
            }
            return Ok(msg.metadata.response_code);
        }
    }
}

/// The primary side: a queue of apices to re-notify. Handing one in never
/// blocks and never fails; a stopped notifier drops it.
#[derive(Clone)]
pub struct Notifier {
    tx: mpsc::UnboundedSender<String>,
}

impl Notifier {
    pub fn new() -> (Self, mpsc::UnboundedReceiver<String>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (Self { tx }, rx)
    }

    /// Ask for `apex` to be notified, whatever its serial says.
    pub fn notify_zone(&self, apex: &str) {
        let _ = self.tx.send(apex.to_owned());
    }
}

/// Pull a zone the moment a NOTIFY asked for it. Runs on every node that
/// serves DNS: a NOTIFY can arrive at any of them, and a node that is not the
/// leader still has to act on one it accepted.
pub(crate) async fn run_pulls(
    client: Client,
    cache: SecondaryCache,
    mut pulled: mpsc::UnboundedReceiver<(String, Option<IpAddr>)>,
    shutdown: CancellationToken,
) {
    while let Some((apex, from)) = pulled.recv().await {
        if shutdown.is_cancelled() {
            break;
        }
        crate::transfer_one(&client, &cache, &apex, from).await;
    }
}

/// Send NOTIFYs for zones whose serial moved. Runs while this node is the
/// leader, beside the reconciler that moves them, so one node notifies for the
/// whole cloud. `active` is the leadership token: when it fires the loop stops
/// with the leadership.
pub(crate) async fn run_notifies(
    client: Client,
    mut events: broadcast::Receiver<pistack_types::Event>,
    mut forced: mpsc::UnboundedReceiver<String>,
    active: CancellationToken,
) {
    let mut window: Option<tokio::time::Instant> = None;
    loop {
        let due = async {
            match window {
                Some(t) => tokio::time::sleep_until(t).await,
                None => std::future::pending::<()>().await,
            }
        };
        tokio::select! {
            _ = active.cancelled() => break,
            // A caller asked for this zone by name.
            Some(apex) = forced.recv() => {
                notify_one(&client, &apex).await;
            }
            ev = events.recv() => match ev {
                Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {
                    // The reconcile pass may not have run yet; give it the
                    // window, then read what the serials actually say.
                    window.get_or_insert_with(|| tokio::time::Instant::now() + COALENSE);
                }
                Err(broadcast::error::RecvError::Closed) => break,
            },
            _ = due => {
                window = None;
                notify_moved(&client).await;
            }
        }
    }
}

/// Every primary zone whose serial has moved since it was last acknowledged,
/// notified to the addresses its secondaries list.
pub(crate) async fn notify_moved(client: &Client) {
    let zones = match client.zones().list(None) {
        Ok(z) => z,
        Err(e) => {
            tracing::warn!(error = %e, "beacon: notify pass");
            return;
        }
    };
    let mut todo = Vec::new();
    for z in zones {
        if !matches!(z.spec.kind, ZoneKind::Primary) || z.status.serial == 0 {
            continue;
        }
        let last = z
            .meta
            .annotations
            .get(NOTIFIED_KEY)
            .and_then(|s| s.parse::<u32>().ok());
        if last == Some(z.status.serial) {
            continue;
        }
        todo.push((z.meta.id.clone(), z.spec.name.clone(), z.status.serial, z));
    }
    notify_zones(client, todo).await;
}

/// Notify one named zone now, whether or not its serial moved. Returns the
/// number of peers asked.
pub async fn notify_one(client: &Client, apex: &str) -> usize {
    let Ok(Some(z)) = client.zone_by_apex(apex) else {
        return 0;
    };
    if !matches!(z.spec.kind, ZoneKind::Primary) || z.status.serial == 0 {
        return 0;
    }
    let n = peers_for(client, &z).map(|p| p.len()).unwrap_or(0);
    notify_zones(
        client,
        vec![(z.meta.id.clone(), z.spec.name.clone(), z.status.serial, z)],
    )
    .await;
    n
}

async fn notify_zones(
    client: &Client,
    todo: Vec<(pistack_types::Id, String, u32, crate::types::Zone)>,
) {
    let mut joins = Vec::new();
    let acked: Arc<Mutex<HashSet<(String, SocketAddr)>>> = Arc::new(Mutex::new(HashSet::new()));
    for (id, apex, serial, zone) in todo.iter() {
        let peers = match peers_for(client, zone) {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!(zone = %apex, error = %e, "beacon: notify peers");
                Vec::new()
            }
        };
        for to in peers {
            let bytes = match notify_bytes(next_id(), apex, *serial) {
                Ok(b) => b,
                Err(e) => {
                    tracing::warn!(zone = %apex, error = %e, "beacon: notify encode");
                    continue;
                }
            };
            let (acked, apex, id) = (Arc::clone(&acked), apex.clone(), id.clone());
            joins.push(tokio::spawn(async move {
                let ok = send_notify(&bytes, to, &acked, &apex, &CancellationToken::new()).await;
                if ok {
                    acked.lock().insert((apex, to));
                }
                (id, ok)
            }));
        }
    }
    let mut done: HashMap<pistack_types::Id, bool> = HashMap::new();
    for j in joins {
        match j.await {
            Ok((id, ok)) => *done.entry(id).or_insert(true) &= ok,
            Err(e) => tracing::debug!(error = %e, "beacon: notify task"),
        }
    }
    for (id, apex, serial, _) in todo {
        match done.get(&id) {
            // Acknowledged by everyone the zone names. A zone with no secondaries
            // has nobody to wait for and is marked the same way.
            Some(true) | None => record(client, &id, serial),
            // Unanswered: leave the mark where it is. The reconcile timer
            // brings the next pass, which will try again.
            _ => tracing::debug!(zone = %apex, serial, "beacon: notify unacknowledged"),
        }
    }
}

/// Write the acknowledged serial back, so the next pass leaves the zone alone
/// until its serial moves again.
fn record(client: &Client, id: &pistack_types::Id, serial: u32) {
    tracing::info!(serial, "beacon: secondaries notified");
    let want = serial.to_string();
    let _ = client.zones().modify(id, |z| {
        z.meta.annotations.insert(NOTIFIED_KEY.into(), want.clone());
        Ok(())
    });
}

fn next_id() -> u16 {
    use rand::Rng;
    rand::thread_rng().gen()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ZoneSpec;
    use pistack_store::Store;
    use pistack_types::{kind, Meta};

    fn client() -> Client {
        Client::new(Store::in_memory().unwrap())
    }

    fn secondary(c: &Client, name: &str, apex: &str, primaries: &[&str]) -> crate::types::Zone {
        c.create_zone(
            Meta::new(kind::ZONE, name, None),
            ZoneSpec {
                name: apex.into(),
                kind: ZoneKind::Secondary {
                    primaries: primaries.iter().map(|s| s.to_string()).collect(),
                },
                ..ZoneSpec::default()
            },
        )
        .unwrap()
    }

    #[test]
    fn addresses_accept_both_forms() {
        assert_eq!(
            parse_addr("10.0.0.1", 53).unwrap(),
            "10.0.0.1:53".parse().unwrap()
        );
        assert_eq!(
            parse_addr("127.0.0.1:15353", 53).unwrap(),
            "127.0.0.1:15353".parse().unwrap()
        );
        assert_eq!(
            parse_addr("[::1]:5353", 53).unwrap(),
            "[::1]:5353".parse().unwrap()
        );
        assert!(parse_addr("ns1.example.cloud", 53).is_err());
    }

    fn primary(c: &Client, name: &str, apex: &str, also: &[&str]) -> crate::types::Zone {
        c.create_zone(
            Meta::new(kind::ZONE, name, None),
            ZoneSpec {
                name: apex.into(),
                also_notify: also.iter().map(|s| s.to_string()).collect(),
                ..ZoneSpec::default()
            },
        )
        .unwrap()
    }

    #[test]
    fn peers_come_from_secondary_primaries() {
        let c = client();
        secondary(&c, "sec-a", "a.cloud.", &["127.0.0.1:15353"]);
        secondary(&c, "sec-b", "b.cloud.", &["10.0.0.2"]);
        assert_eq!(
            peers_for(&c, &primary(&c, "prim-a", "a.cloud.", &[])).unwrap(),
            vec!["127.0.0.1:15353".parse::<SocketAddr>().unwrap()]
        );
        // A bare address means the standard port.
        assert_eq!(
            peers_for(&c, &primary(&c, "prim-b", "b.cloud.", &[])).unwrap(),
            vec!["10.0.0.2:53".parse::<SocketAddr>().unwrap()]
        );
        assert!(peers_for(&c, &primary(&c, "prim-c", "other.cloud.", &[]))
            .unwrap()
            .is_empty());
    }

    /// A secondary that is an outside DNS server holds nothing in the store, so
    /// the zone names it directly. Both lists are asked at once and an address
    /// named twice is asked once.
    #[test]
    fn also_notify_addresses_are_added() {
        let c = client();
        secondary(&c, "sec", "d.cloud.", &["10.0.0.9", "10.0.0.10"]);
        let z = primary(&c, "prim", "d.cloud.", &["198.51.100.7", "10.0.0.10"]);
        assert_eq!(
            peers_for(&c, &z).unwrap(),
            vec![
                "198.51.100.7:53".parse::<SocketAddr>().unwrap(),
                "10.0.0.10:53".parse::<SocketAddr>().unwrap(),
                "10.0.0.9:53".parse::<SocketAddr>().unwrap(),
            ]
        );
    }

    #[test]
    fn notify_message_is_a_notify_for_the_apex() {
        let bytes = notify_bytes(0xabcd, "example.cloud.", 9).unwrap();
        let msg = Message::from_vec(&bytes).unwrap();
        assert_eq!(msg.metadata.id, 0xabcd);
        assert_eq!(msg.metadata.op_code, OpCode::Notify);
        assert_eq!(msg.metadata.message_type, MessageType::Query);
        assert!(msg.metadata.authoritative);
        assert_eq!(msg.queries.len(), 1);
        assert_eq!(msg.queries[0].name().to_ascii(), "example.cloud.");
        assert_eq!(msg.queries[0].query_type(), RecordType::SOA);
        assert_eq!(msg.answers.len(), 1);
        match &msg.answers[0].data {
            RData::SOA(soa) => assert_eq!(soa.serial, 9),
            other => panic!("the hint should be SOA, got {other:?}"),
        }
        assert_eq!(asked_apex(&msg).as_deref(), Some("example.cloud."));
    }

    #[test]
    fn only_a_listed_primary_may_notify() {
        let c = client();
        secondary(&c, "sec", "x.cloud.", &["127.0.0.1"]);
        let msg = Message::from_vec(&notify_bytes(1, "x.cloud.", 3).unwrap()).unwrap();
        // A NOTIFY arrives from an ephemeral source port, so the address a
        // secondary lists is matched on host alone: two primaries packed onto
        // one host are one principal as far as a NOTIFY can tell.
        assert!(notify_allowed(&c, &msg, "127.0.0.1:53".parse().unwrap()).unwrap());
        assert!(notify_allowed(&c, &msg, "127.0.0.1:15353".parse().unwrap()).unwrap());
        // Another host, a zone we do not hold, and a plain query all fail.
        assert!(!notify_allowed(&c, &msg, "10.9.9.9:53".parse().unwrap()).unwrap());
        let other = Message::from_vec(&notify_bytes(1, "y.cloud.", 3).unwrap()).unwrap();
        assert!(!notify_allowed(&c, &other, "127.0.0.1:53".parse().unwrap()).unwrap());
        let q = Message::query();
        assert!(!notify_allowed(&c, &q, "127.0.0.1:53".parse().unwrap()).unwrap());
    }

    #[tokio::test]
    async fn an_unanswered_notify_is_not_recorded() {
        let c = client();
        let z = c
            .create_zone(
                Meta::new(kind::ZONE, "prim", None),
                ZoneSpec {
                    name: "gone.cloud.".into(),
                    ..ZoneSpec::default()
                },
            )
            .unwrap();
        c.reconcile_zone(&z.meta.id).unwrap();
        // One listed peer, nothing listening at it. The retries burn the
        // backoff, which is the point: it must end, and it must end unmarked.
        secondary(&c, "gone-secondary", "gone.cloud.", &["127.0.0.1:1"]);
        notify_moved(&c).await;
        let after = c.zones().get(&z.meta.id).unwrap();
        assert!(
            !after.meta.annotations.contains_key(NOTIFIED_KEY),
            "an unanswered notify must not be recorded: {:?}",
            after.meta.annotations
        );
    }

    #[tokio::test]
    async fn a_zone_nobody_listed_is_marked_immediately() {
        let c = client();
        let z = c
            .create_zone(
                Meta::new(kind::ZONE, "solo", None),
                ZoneSpec {
                    name: "solo.cloud.".into(),
                    ..ZoneSpec::default()
                },
            )
            .unwrap();
        c.reconcile_zone(&z.meta.id).unwrap();
        notify_moved(&c).await;
        let after = c.zones().get(&z.meta.id).unwrap();
        assert_eq!(
            after.meta.annotations.get(NOTIFIED_KEY).map(|s| s.as_str()),
            Some("1")
        );
        // A second pass has nothing to do.
        notify_moved(&c).await;
        assert_eq!(
            c.zones().get(&z.meta.id).unwrap().meta.revision,
            after.meta.revision
        );
    }
}
