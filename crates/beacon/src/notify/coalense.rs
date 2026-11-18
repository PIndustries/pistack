// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::time::Duration;

/// A zone event waits this long for the rest of its burst before the notify
/// pass runs. Serials move on the reconcile pass, so one window covers one
/// bump however many writes caused it.
pub const COALENSE: Duration = Duration::from_millis(75);
