// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `Id`: a UUIDv7 with a short kind prefix, e.g. `mach-018f...`.

use serde::{Deserialize, Serialize};
use std::fmt;
use uuid::Uuid;

/// Every id is a UUIDv7 (time-ordered) rendered with a short kind prefix,
/// e.g. `mach-018f...`, `proj-018f...`. The prefix makes logs readable and
/// stops a volume id being passed where a machine id was meant.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Id {
    pub kind: &'static str,
    pub uuid: Uuid,
}

impl Id {
    pub fn new(kind: &'static str) -> Self {
        Self {
            kind,
            uuid: Uuid::now_v7(),
        }
    }
    pub fn parse(kind: &'static str, s: &str) -> Option<Self> {
        let rest = s.strip_prefix(kind)?.strip_prefix('-')?;
        Uuid::parse_str(rest).ok().map(|uuid| Self { kind, uuid })
    }
}

impl fmt::Display for Id {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}-{}", self.kind, self.uuid)
    }
}
impl fmt::Debug for Id {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}
impl Serialize for Id {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}
impl<'de> Deserialize<'de> for Id {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        let (kind, rest) = s
            .split_once('-')
            .ok_or_else(|| serde::de::Error::custom("id must be <kind>-<uuid>"))?;
        let uuid = Uuid::parse_str(rest).map_err(serde::de::Error::custom)?;
        // Leak the kind so we can hold a &'static str. Kinds are a small closed set.
        let kind: &'static str = Box::leak(kind.to_owned().into_boxed_str());
        Ok(Self { kind, uuid })
    }
}
