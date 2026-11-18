// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::Host;
use pistack_types::{ApiError, ApiResult, Phase};

use super::evacuate_options::EvacuateOptions;
use super::fence_failed::fence_failed;
use super::fencer::Fencer;
use super::needs_proof::needs_proof;
use super::proof::Proof;
use super::still_reporting::still_reporting;

/// Prove `host` is off, so its machines may start elsewhere without a
/// second copy.
///
/// A host that stopped reporting may be evacuated: it is gone, which is what
/// evacuate is for. A host that still reports may not, even once disabled —
/// that is a host that works, and a working host is emptied with drain.
///
/// A host that is down and has power control is fenced: the server powers it
/// off and reads the off state back, and no confirmation is needed. A host
/// that still reports needs `confirm_dead` before the server will touch its
/// power at all, because disabling a host is not a statement that it is dead.
///
/// A power-off that fails or cannot be confirmed within [`FENCE_TIMEOUT`]
/// refuses the evacuation and moves nothing, unless `confirm_dead` was given:
/// a BMC that died with its host is a dead host, and an operator who has
/// looked at it is allowed to say so. The report then says the power-off was
/// tried and failed, and on what grounds. Without the confirmation nothing
/// moves, and the refusal says why. Returns the proof used and, when a
/// power-off was attempted and did not happen, the note describing it.
pub(crate) async fn prove_off(
    host: &Host,
    opts: EvacuateOptions,
    fencer: Option<&dyn Fencer>,
) -> ApiResult<(Proof, Option<String>)> {
    // A host still doing its work is emptied with drain, which copies
    // machines off, not with an evacuation that abandons them here.
    let down = host.status.phase == Phase::Error;
    if !down && !host.spec.disabled {
        return Err(ApiError::conflict(format!(
            "host {} is up. Evacuate only acts on a host that is down, or one you have disabled. \
             Use drain to empty a working host",
            host.meta.name
        )));
    }
    // Disabled says no new work lands here; it does not say the host is off.
    // Only a host that stopped reporting may be powered off unasked.
    if !down && !opts.confirm_dead {
        return Err(still_reporting(host));
    }
    if let (Some(pc), Some(fencer)) = (host.spec.power_control.as_ref(), fencer) {
        // Power control is configured but the fencer cannot reach it (no
        // secret, no tool): that is a fault, and the operator's word only
        // stands in for it when they have explicitly said the host is dead.
        let transport = pc.kind.clone();
        if let Err(e) = fencer.fence(host).await {
            let why = fence_failed(host, &transport, &e.message);
            if !opts.confirm_dead {
                return Err(ApiError::conflict(format!(
                    "{why}, so nothing was evacuated and its machines were not started a second \
                     time. Confirm the host is dead (confirm_dead, \
                     `pi host evacuate --confirm-dead`) to evacuate anyway"
                )));
            }
            return Ok((Proof::Operator, Some(why)));
        }
        return Ok((Proof::PowerOff, None));
    }
    if opts.confirm_dead {
        return Ok((Proof::Operator, None));
    }
    Err(needs_proof(host))
}
