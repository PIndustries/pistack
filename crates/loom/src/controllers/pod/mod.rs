// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The pod controller: a pod becomes a forge `Machine` of kind `Container`.
//!
//! ```text
//! Pod Pending ──create──▶ Machine{kind: Container}  ──forge──▶ Ready
//!     ▲                                                          │
//!     └── mirror: Ready/On → Running (pod_ip, host); Error → Failed ┘
//! ```
//!
//! # The machine → anvil contract
//!
//! The machine's `spec.user_data` is the JSON of a [`PodSandbox`]: the pod's
//! identity plus its full container spec. `anvil`'s youki runtime
//! deserialises it to build the OCI bundle(s): one sandbox (network
//! namespace, the machine's `addresses`) and one container per entry in
//! `containers`, in order, sharing the sandbox namespaces. `spec.image` is
//! the first container's image for display and image pre-pull only; the
//! runtime takes each container's image from the sandbox document.
//!
//! What the runtime reports back through the ordinary `Machine.status`:
//! * `phase = Ready, power = On` while the containers run;
//! * when a container's main process exits, `power = Off` plus a condition
//!   `ContainerExited{status: true, reason: "<exit code>", message: "<name>"}`
//!   (`reason` is the decimal exit code of the first container to exit);
//! * `phase = Error` when the sandbox or a container cannot be started;
//! * `status.probes` carries the host agent's liveness and readiness
//!   streaks for each container.
//! * a `Network` condition with status false means the sandbox is up and
//!   the recorded address is not on an interface. The pod stays `Running`,
//!   `pod_ip` stays empty, and Ready is false.
//!
//! The controller maps that to the pod: `Running`, `Succeeded` (exit 0),
//! `Failed`, and restarts per `restart_policy` by bumping the machine's
//! `spec.restarts` (the same knob `POST /machines/{id}/reboot` uses) with an
//! exponential backoff capped at five minutes. A container with no
//! readiness probe is ready once it is running. A readiness probe holds
//! the pod's Ready condition false until its success streak reaches
//! `success_threshold`. A liveness streak that reaches `failure_threshold`
//! restarts that container the same way, without the exit backoff, and
//! bumps only that container's restart count. `http_get`, `tcp_socket`, and
//! `exec` all use those thresholds. An exec probe the runtime cannot run is
//! reported unsupported and does not restart the container.
//!
//! A pod that names a slab volume stays Pending until every such volume is
//! attached on one node and the status records a host path or a device.
//! The machine is created only then, pinned to that node, with the path in
//! the sandbox document. If a volume attaches after the pod is already
//! running without it, the pod is marked not Ready and the machine restart
//! counter is bumped so the runtime creates the containers again.

pub(in crate::controllers::pod) mod controller;
pub(in crate::controllers::pod) mod controller_impl;
pub(in crate::controllers::pod) mod document;
pub(in crate::controllers::pod) mod networks;
pub(in crate::controllers::pod) mod ops;
pub(in crate::controllers::pod) mod probes;
pub(in crate::controllers::pod) mod projections;
pub(in crate::controllers::pod) mod slab;

pub use controller::PodController;
pub use document::{PodIdentity, PodSandbox, ProjectedFiles};
pub use document::{DEFAULT_NETWORK, EXITED_CONDITION, SANDBOX_API_VERSION, SANDBOX_KIND};
pub use networks::{exact_size_name, pick_networks};
