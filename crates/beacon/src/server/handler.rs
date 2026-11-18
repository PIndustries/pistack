// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::Client;
use crate::notify;
use crate::types::KINDS;
use async_trait::async_trait;
use hickory_proto::op::{Metadata, OpCode, ResponseCode};
use hickory_proto::rr::{LowerName, Name, Record, RecordType};
use hickory_server::net::runtime::Time;
use hickory_server::net::xfer::Protocol;
use hickory_server::server::{Request, RequestHandler, ResponseHandler, ResponseInfo};
use hickory_server::zone_handler::MessageResponseBuilder;
use parking_lot::{Mutex, RwLock};
use pistack_types::module::CancellationToken;
use pistack_types::Event;
use std::net::IpAddr;
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc};

use super::answer::Answer;
use super::catalog::Catalog;
use super::pulled::Pulled;
use super::secondary_cache::SecondaryCache;

/// Store-backed [`RequestHandler`] with an invalidating cache.
#[derive(Clone)]
pub struct Handler {
    pub(crate) client: Client,
    pub(crate) cache: Arc<RwLock<Option<Arc<Catalog>>>>,
    pub(crate) secondaries: SecondaryCache,
    /// Apices an accepted NOTIFY asked for, handed to the pull loop.
    pub(crate) refresh: Arc<Mutex<Option<mpsc::UnboundedSender<Pulled>>>>,
}

impl Handler {
    pub fn new(client: Client, secondaries: SecondaryCache) -> Self {
        Self {
            client,
            cache: Arc::new(RwLock::new(None)),
            secondaries,
            refresh: Arc::new(Mutex::new(None)),
        }
    }

    /// Where an accepted NOTIFY leaves its zone. Set once at start; a handler
    /// without one still answers, it simply has nobody to wake.
    pub fn with_refresh(self, tx: mpsc::UnboundedSender<(String, Option<IpAddr>)>) -> Self {
        *self.refresh.lock() = Some(tx);
        self
    }

    /// Drop the compiled catalog; the next query rebuilds it.
    pub fn invalidate(&self) {
        *self.cache.write() = None;
    }

    pub(crate) fn catalog(&self) -> anyhow::Result<Arc<Catalog>> {
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
        let primary = catalog.find(name);
        let secondary = self.secondaries.find(name);
        let zone = match (primary, secondary) {
            (Some(p), Some(s)) => {
                if s.apex.num_labels() > p.apex.num_labels() {
                    Some(s)
                } else {
                    // Prefer the borrowed primary by looking up again via clone path:
                    Some(p.clone())
                }
            }
            (Some(p), None) => Some(p.clone()),
            (None, Some(s)) => Some(s),
            (None, None) => None,
        };
        Ok(zone.map(|z| z.lookup(name, qtype)))
    }

    /// AXFR answer for an exact zone apex, or `None` when we do not own it.
    pub fn resolve_axfr(&self, apex: &Name) -> anyhow::Result<Option<Answer>> {
        let catalog = self.catalog()?;
        let zone = catalog
            .find_apex(apex)
            .cloned()
            .or_else(|| self.secondaries.find_apex(apex));
        Ok(zone.map(|z| Answer {
            code: ResponseCode::NoError,
            answers: z.axfr(),
            authorities: Vec::new(),
        }))
    }
}

#[async_trait]
impl RequestHandler for Handler {
    async fn handle_request<R: ResponseHandler, T: Time>(
        &self,
        request: &Request,
        mut response_handle: R,
    ) -> ResponseInfo {
        // hickory has no notion of NOTIFY and would answer NOTIMP to every
        // one, so it is settled here, before the query path.
        if request.metadata.op_code == OpCode::Notify {
            return self.handle_notify(request, &mut response_handle).await;
        }
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
    /// Accept or refuse a NOTIFY, and hand an accepted one to the pull loop.
    pub(crate) async fn handle_notify<R: ResponseHandler>(
        &self,
        request: &Request,
        response_handle: &mut R,
    ) -> ResponseInfo {
        let src = request.src();
        let message = match hickory_proto::op::Message::from_vec(request.as_slice()) {
            Ok(m) => m,
            Err(_) => {
                return notify::answer_notify(request, ResponseCode::FormErr, response_handle)
                    .await;
            }
        };
        let allowed = match notify::notify_allowed(&self.client, &message, src) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(error = %e, "beacon: notify lookup failed");
                false
            }
        };
        if !allowed {
            tracing::debug!(from = %src, "beacon: notify refused");
            return notify::answer_notify(request, ResponseCode::Refused, response_handle).await;
        }
        let apex = notify::asked_apex(&message).unwrap_or_default();
        if let Some(tx) = self.refresh.lock().as_ref() {
            let _ = tx.send((apex.clone(), Some(src.ip())));
        }
        tracing::debug!(zone = %apex, from = %src, "beacon: notify accepted");
        notify::answer_notify(request, ResponseCode::NoError, response_handle).await
    }

    /// Decide what to answer, or which error code to return.
    pub(crate) fn classify(&self, request: &Request) -> Result<Answer, ResponseCode> {
        if request.metadata.op_code != OpCode::Query {
            return Err(ResponseCode::NotImp);
        }
        let info = request.request_info().map_err(|_| ResponseCode::FormErr)?;
        let qtype = info.query.query_type();
        let lower: &LowerName = info.query.name();
        let mut name = Name::from(lower);
        name.set_fqdn(true);

        if qtype == RecordType::IXFR {
            return Err(ResponseCode::NotImp);
        }
        if qtype == RecordType::AXFR {
            // AXFR is TCP-only (RFC 5936). Refuse UDP so clients fall back.
            if request.protocol() != Protocol::Tcp {
                return Err(ResponseCode::Refused);
            }
            return match self.resolve_axfr(&name) {
                Ok(Some(a)) => Ok(a),
                Ok(None) => Err(ResponseCode::Refused),
                Err(e) => {
                    tracing::error!(error = %e, "beacon: axfr failed");
                    Err(ResponseCode::ServFail)
                }
            };
        }

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
