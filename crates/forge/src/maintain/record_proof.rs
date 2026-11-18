// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::Host;
use pistack_store::Store;
use pistack_types::kind;

use super::proof::Proof;

/// Write an audit record naming the proof an evacuation used, alongside the
/// `evacuate` action the gateway already records. When a power-off was tried
/// and failed the reason rides along as an audit note, so the record says that
/// the fence was attempted and did not happen, not only whose word was taken
/// instead. A store error is logged, not returned: on a follower the
/// write is refused, and the evacuation itself has already happened.
pub(crate) fn record_proof(
    store: &Store,
    host: &Host,
    proof: Proof,
    power_off_failed: Option<&str>,
    moved: usize,
) {
    let mut meta = pistack_types::Meta::new("audit", "audit", None);
    meta.name = meta.id.to_string();
    let mut notes = std::collections::BTreeMap::new();
    if let Some(why) = power_off_failed {
        notes.insert("power_off_failed".to_owned(), why.to_owned());
    }
    let event = gate::audit::AuditEvent {
        meta,
        spec: gate::audit::AuditSpec {
            method: "POST".into(),
            path: format!("/compute/hosts/{}/evacuate", host.meta.id),
            action: format!("evacuate:proof_{}", proof.audit_tag()),
            kind: kind::HOST.into(),
            resource: host.meta.id.to_string(),
            notes,
            ..Default::default()
        },
        status: gate::audit::AuditStatus {},
    };
    if let Err(e) = store.collection::<gate::audit::AuditEvent>().create(event) {
        tracing::debug!(host = %host.meta.id, error = %e, "forge: evacuation proof was not audited");
        return;
    }
    tracing::info!(
        host = %host.meta.id,
        proof = proof.as_str(),
        moved,
        power_off_failed = power_off_failed.unwrap_or("none"),
        "forge: host evacuated"
    );
}
