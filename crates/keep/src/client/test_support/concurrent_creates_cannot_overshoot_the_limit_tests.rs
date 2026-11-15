// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::limited::limited;
use super::mk::mk;
use pistack_types::{kind, Id};

#[cfg(test)]
#[test]
fn concurrent_creates_cannot_overshoot_the_limit() {
    let c = limited(5, 10);
    let p = Id::new(kind::PROJECT);
    let handles: Vec<_> = (0..16)
        .map(|i| {
            let c = c.clone();
            let p = p.clone();
            std::thread::spawn(move || c.create_secret(mk(&format!("s{i}"), &p), b"x").is_ok())
        })
        .collect();
    let ok = handles
        .into_iter()
        .map(|h| h.join().unwrap())
        .filter(|ok| *ok)
        .count();
    assert_eq!(ok, 5);
    assert_eq!(c.list_secrets(Some(&p)).unwrap().len(), 5);
}
