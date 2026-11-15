// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::probe::book::new_track::new_track;
use crate::probe::book::probe_kind::ProbeKind;
use crate::probe::book::track::Track;
use crate::probe::spec::container_probes::ContainerProbes;
use chrono::{DateTime, Utc};

pub(super) fn tracks_from(specs: Vec<ContainerProbes>, now: DateTime<Utc>) -> Vec<Track> {
    let mut tracks = Vec::new();
    for spec in specs {
        if let Some(liveness) = spec.liveness {
            tracks.push(new_track(
                spec.container.clone(),
                ProbeKind::Liveness,
                liveness,
                now,
            ));
        }
        if let Some(readiness) = spec.readiness {
            tracks.push(new_track(
                spec.container,
                ProbeKind::Readiness,
                readiness,
                now,
            ));
        }
    }
    tracks
}
