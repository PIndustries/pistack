// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use chrono::TimeDelta;

/// measured on the stamps of the points a series keeps, so nothing beyond this window
/// can ever be waited for. See [`MAX_HOLD`].
pub const WINDOW_POINTS: usize = 240;
/// No point is kept longer than this.
pub const WINDOW_AGE: chrono::Duration = chrono::Duration::minutes(60);
/// Series one server keeps. A machine with one disk and one interface is
/// about a dozen, so this is room for some four thousand machines.
pub const MAX_SERIES: usize = 50_000;
/// A series with no sample for this long is left out of `/metrics` and of
/// alarms: its machine or host is gone or has stopped reporting.
pub const STALE_AFTER: chrono::Duration = chrono::Duration::minutes(5);
/// How long an alarm's `last_value` may lag while its phase stays the same.
/// Each change of it is a write to the store.
pub const ALARM_REFRESH: chrono::Duration = chrono::Duration::seconds(60);
/// The longest hold an alarm may ask for: a `WINDOW_POINTS` run of samples at the
/// [`WINDOW_AGE`] limit. An alarm asking for more could never fire, so it is refused
/// where it is made rather than left pending forever.
pub const MAX_HOLD: TimeDelta = WINDOW_AGE;
