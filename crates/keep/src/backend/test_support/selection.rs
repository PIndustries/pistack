// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::collections::BTreeMap;

use crate::backend::*;
use crate::error::{KeepError, Result};

pub(in crate::backend) fn parse(pairs: &[(&str, &str)]) -> Result<KeyBackend> {
    let env: BTreeMap<String, String> = pairs
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect();
    KeyBackend::from_lookup(|k| env.get(k).cloned())
}

pub(in crate::backend) fn err(pairs: &[(&str, &str)]) -> String {
    match parse(pairs) {
        Ok(b) => panic!("expected an error, got {b:?}"),
        Err(e) => {
            assert!(matches!(e, KeepError::Backend(_)), "{e}");
            e.to_string()
        }
    }
}
