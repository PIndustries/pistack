// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::migrate_dest::migrate_dest;
use crate::types::Host;
use crate::types::Machine;
use pistack_rpc::proto::CommandKind;
use pistack_rpc::proto::MigrateRequest;
use pistack_rpc::CommandQueue;
use pistack_store::Store;
use pistack_types::Phase;
use pistack_types::{ApiError, ApiResult};
/// Queue a live migration of `machine` onto `dest`.
///
/// The command goes to the source node's agent. A refusal leaves the
/// placement alone. Phase stays `Updating` so the reconciler does not start
/// the guest again. `status.host` changes when the destination reports the
/// machine.
pub fn begin_migration(
    store: &Store,
    queue: &CommandQueue,
    machine: &Machine,
    dest: &Host,
) -> ApiResult<Machine> {
    let source_id =
        machine.status.host.clone().ok_or_else(|| {
            ApiError::conflict(format!("machine {} has no host", machine.meta.id))
        })?;
    if source_id == dest.meta.id {
        return Err(ApiError::conflict(
            "destination is the machine's current host",
        ));
    }
    if dest.status.phase != Phase::Ready {
        return Err(ApiError::conflict(format!(
            "host {} is not ready",
            dest.meta.name
        )));
    }
    // Underlay is required whether or not `spec.networks` names one.
    let dest_uri = migrate_dest(dest)?;
    let source = store.collection::<Host>().get(&source_id)?;
    if source.spec.node_name.is_empty() {
        return Err(ApiError::unprocessable(format!(
            "host {} has no node name",
            source.meta.name
        )));
    }
    // The hypervisor refuses a migration whose disks stay behind. Asking is
    // a certain failure, so it is refused here instead.
    if let Some(reason) = crate::maintain::disk_holds_machine(store, machine, &source) {
        return Err(ApiError::unprocessable(reason));
    }
    let payload = MigrateRequest {
        id: machine.meta.id.clone(),
        dest: dest_uri,
    };
    let body = serde_json::to_value(&payload).map_err(|e| ApiError::internal(e.to_string()))?;
    let ticket = queue.enqueue(&source.spec.node_name, CommandKind::MigrateMachine, body)?;
    let updated = store
        .collection::<Machine>()
        .modify(&machine.meta.id, |m| {
            m.status.phase = Phase::Updating;
            m.status.message = format!("migrating {}", dest.meta.id);
            Ok(())
        })
        .map_err(ApiError::from)?;
    // The HTTP handler must return now. The agent answers when the domain is
    // on the destination; until then the machine stays `Updating` and the
    // store still names its old host, because that is the only host forge
    // has heard from. Whatever the answer is, it is not proof the guest
    // stopped running, so the machine goes back to `Ready` on the host the
    // store holds and a later drain starts the move again. A report from
    // either host settles which one it is on.
    let store = store.clone();
    let machine_id = machine.meta.id.clone();
    let expect = format!("migrating {}", dest.meta.id);
    tokio::spawn(async move {
        let word = match ticket.wait().await {
            Ok(result) if result.ok => return,
            Ok(result) if result.message.is_empty() => "the host refused it".to_owned(),
            Ok(result) => result.message,
            Err(e) => {
                // No answer. The guest may still be copying, or may be
                // running on the destination with the answer lost. The host
                // the store names is the last one forge heard from, so the
                // machine is left ready there, which is where it can be
                // reached. A report from either host settles it, and a later
                // drain can start the move again.
                tracing::warn!(machine = %machine_id, error = %e, "forge: no answer to a live migration");
                format!("the host never answered ({e})")
            }
        };
        let _ = store.collection::<Machine>().modify(&machine_id, |m| {
            if m.status.phase == Phase::Updating && m.status.message == expect {
                m.status.phase = Phase::Ready;
                m.status.message = format!("migrate did not happen: {word}");
            }
            Ok(())
        });
    });
    Ok(updated)
}
