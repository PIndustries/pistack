// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::delete_request::DeleteRequest;

/// A parsed DeleteObjects request body.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DeleteObjectsBody {
    pub objects: Vec<DeleteRequest>,
    /// `<Quiet>` outside `<Object>`, which applies to every one without its own.
    pub quiet: bool,
}
