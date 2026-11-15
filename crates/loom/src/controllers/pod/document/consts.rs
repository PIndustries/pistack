// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
pub const SANDBOX_API_VERSION: &str = "loom.pistack.io/v1";
pub const SANDBOX_KIND: &str = "PodSandbox";

/// Machine condition anvil sets when a container exits; `reason` is the
/// decimal exit code.
pub const EXITED_CONDITION: &str = "ContainerExited";

/// Pod condition set from liveness probes. Absent when the pod has none.
pub(in crate::controllers::pod) const LIVENESS_CONDITION: &str = "Liveness";

/// Network attached when the pod names none and the project has a network
/// by this name. Otherwise the project's only non-external network is used
/// (see [`pick_networks`]).
pub const DEFAULT_NETWORK: &str = "default";
