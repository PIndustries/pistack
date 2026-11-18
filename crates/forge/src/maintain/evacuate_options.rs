// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use serde::{Deserialize, Serialize};

/// Body of `POST /hosts/{id}/evacuate`.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct EvacuateOptions {
    /// The operator says the host is dead. Required every time a host has
    /// no power control, whether or not it is already marked down. Not
    /// needed for a host the server can power off and confirm.
    #[serde(default)]
    pub confirm_dead: bool,
    /// Accept that each machine starts again from its image without the
    /// root disk it had. No machine is evacuated without it.
    #[serde(default)]
    pub rebuild_from_image: bool,
}
