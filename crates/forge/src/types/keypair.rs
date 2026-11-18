// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::{Kind, Meta, Resource};
use serde::{Deserialize, Serialize};
use std::ops::{Deref, DerefMut};

use super::keypair_kind::KEYPAIR_KIND;
use super::keypair_spec::KeypairSpec;
use super::keypair_status::KeypairStatus;

resource_kind!(
    /// A named SSH public key in a project.
    Keypair,
    KeypairSpec,
    KeypairStatus,
    KEYPAIR_KIND
);
