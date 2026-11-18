// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::io;

use super::max_redirects::MAX_REDIRECTS;
use super::validate_source_url::validate_source_url;

pub(crate) fn redirect_action(attempt: reqwest::redirect::Attempt) -> reqwest::redirect::Action {
    if attempt.previous().len() >= MAX_REDIRECTS {
        return attempt.error(io::Error::other("image import: too many redirects"));
    }
    match validate_source_url(attempt.url().as_str()) {
        Ok(()) => attempt.follow(),
        Err(e) => attempt.error(io::Error::other(format!("image import: {e}"))),
    }
}
