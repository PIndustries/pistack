// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `Spec`: implement on a spec type to make `Resource<ThisSpec, _>` storable.

use serde::{de::DeserializeOwned, Serialize};

/// Implement this on a spec type to make `Resource<ThisSpec, AnyStatus>` a
/// storable [`Kind`](super::kind_trait::Kind).
///
/// ```ignore
/// #[derive(Serialize, Deserialize, Clone)] struct MachineSpec { .. }
/// impl Spec for MachineSpec { const KIND: &'static str = "mach"; }
/// type Machine = Resource<MachineSpec, MachineStatus>; // now a Kind
/// ```
pub trait Spec: Serialize + DeserializeOwned + Clone + Send + Sync + 'static {
    const KIND: &'static str;
}
