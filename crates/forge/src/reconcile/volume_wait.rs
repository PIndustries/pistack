// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::{Machine, WAIT_VOLUME_ANNOTATION};

use super::nonempty_annotation::nonempty_annotation;

pub(crate) fn volume_wait(m: &Machine) -> Option<&str> {
    nonempty_annotation(m, WAIT_VOLUME_ANNOTATION)
}
