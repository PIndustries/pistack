// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::Resource;

use super::alarm_spec::AlarmSpec;
use super::alarm_status::AlarmStatus;

pub type Alarm = Resource<AlarmSpec, AlarmStatus>;
