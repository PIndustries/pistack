// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::series_type::Series;

/// The key for `name` with `labels`.
pub fn series(name: &str, labels: &[(&str, &str)]) -> String {
    let mut s = Series::new(name);
    for (key, value) in labels {
        s.labels.insert((*key).to_owned(), (*value).to_owned());
    }
    s.key()
}
