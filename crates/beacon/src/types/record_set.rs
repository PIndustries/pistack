// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::{kind, Kind, Meta, Resource};
use serde::{Deserialize, Serialize};

use super::record_set_spec::RecordSetSpec;
use super::record_set_status::RecordSetStatus;

resource!(
    /// One owner name and one type in a zone.
    RecordSet,
    RecordSetSpec,
    RecordSetStatus,
    kind::RECORD
);
