// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::{kind, Spec};
use serde::{Deserialize, Serialize};

use super::comparison::Comparison;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct AlarmSpec {
    pub name: String,
    /// A series, or a name and some labels that select several, such as
    /// `pistack_machine_cpu_usage_cores{machine="mach-..."}` or just
    /// `pistack_host_load1`. The alarm fires while any of them crosses.
    pub metric: String,
    pub comparison: Comparison,
    pub threshold: f64,
    /// How long the value must keep crossing before the alarm fires, in seconds. `0`,
    /// and no `for_seconds` at all, fires on the first sample that crosses. The wait is
    /// measured on the stamps of the samples themselves, so it runs in the clock of the
    /// series and not the clock of the server deciding it. Longer than [`MAX_HOLD`] is
    /// refused: the window a series keeps could never cover it.
    #[serde(default)]
    pub for_seconds: u32,
}

impl Spec for AlarmSpec {
    const KIND: &'static str = kind::ALARM;
}
