// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `ApiResult<T>`: the result every API handler returns.

use super::api_error::ApiError;

pub type ApiResult<T> = Result<T, ApiError>;
