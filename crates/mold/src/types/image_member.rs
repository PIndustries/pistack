// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::{Kind, Meta, Resource};
use serde::{Deserialize, Serialize};

use super::image_member_spec::ImageMemberSpec;
use super::image_member_status::ImageMemberStatus;
use super::kind_image_member::KIND_IMAGE_MEMBER;

resource_kind!(
    ImageMember,
    ImageMemberSpec,
    ImageMemberStatus,
    KIND_IMAGE_MEMBER
);
