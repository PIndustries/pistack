// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::Resource;

use crate::http::api::Api;

pub(crate) type Res<A> = Resource<A, <A as Api>::Status>;
