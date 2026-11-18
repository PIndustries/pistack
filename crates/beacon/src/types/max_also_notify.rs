// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

/// How many `also_notify` addresses one primary zone may name. A zone that
/// needs more is a anycast set, not a zone with secondaries.
pub const MAX_ALSO_NOTIFY: usize = 16;
