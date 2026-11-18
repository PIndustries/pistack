// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::{Kind, Meta, Resource};
use serde::{Deserialize, Serialize};

use super::image_spec::ImageSpec;
use super::image_status::ImageStatus;
use super::kind_image::KIND_IMAGE;

resource_kind!(Image, ImageSpec, ImageStatus, KIND_IMAGE);
