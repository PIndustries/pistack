// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `ApiError`, the one error shape every HTTP handler returns, and its
//! `ErrorCode`.

mod api_error;
mod api_result;
mod error_code;
#[cfg(test)]
mod tests;

pub use api_error::ApiError;
pub use api_result::ApiResult;
pub use error_code::ErrorCode;
