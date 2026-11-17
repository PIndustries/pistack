// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Which providers the relay module mounts, and how the environment picks
//! between them. A typo in `PISTACK_RELAY_BACKEND` is an error, never a
//! silent fall back to mock.

/// Which providers the relay module mounts, chosen by `PISTACK_RELAY_BACKEND`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum RelayBackend {
    /// Records balancers as ready without moving traffic. Tests and laptops.
    #[default]
    Mock,
    /// In-process L4 forwarder plus the HTTP reverse proxy.
    Forward,
}

impl RelayBackend {
    /// Unset or empty is `Mock`. Anything other than `mock` or `forward`
    /// (any case, surrounding spaces ignored) is an error, so a typo never
    /// leaves balancers on the mock without anyone noticing.
    pub fn parse(value: Option<&str>) -> Result<Self, String> {
        match value.map(str::trim).map(str::to_ascii_lowercase).as_deref() {
            None | Some("") | Some("mock") => Ok(Self::Mock),
            Some("forward") => Ok(Self::Forward),
            Some(other) => Err(format!(
                "PISTACK_RELAY_BACKEND={other:?} is not a relay backend; use mock or forward"
            )),
        }
    }

    /// The backend for an explicit `PISTACK_RELAY_BACKEND` value and the
    /// `PISTACK_FABRIC_BACKEND` value. An explicit value always wins. When
    /// it is unset or empty, a server on a real datapath (`hostnat`,
    /// `overlay` or `ovn`) gets `Forward`, so its balancers move traffic; anything
    /// else keeps `Mock`.
    pub fn choose(value: Option<&str>, fabric: Option<&str>) -> Result<Self, String> {
        let explicit = value.map(str::trim).is_some_and(|v| !v.is_empty());
        if explicit {
            return Self::parse(value);
        }
        match fabric.map(str::trim) {
            Some("hostnat") | Some("overlay") | Some("ovn") => Ok(Self::Forward),
            _ => Ok(Self::Mock),
        }
    }

    /// Read `PISTACK_RELAY_BACKEND`, defaulting from `PISTACK_FABRIC_BACKEND`.
    pub fn from_env() -> Result<Self, String> {
        Self::choose(
            std::env::var("PISTACK_RELAY_BACKEND").ok().as_deref(),
            std::env::var("PISTACK_FABRIC_BACKEND").ok().as_deref(),
        )
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Mock => "mock",
            Self::Forward => "forward",
        }
    }
}
