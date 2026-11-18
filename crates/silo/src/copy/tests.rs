// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::*;

#[test]
fn parse_source_shapes() {
    let s = parse_copy_source("/bkt/a/b.txt").unwrap();
    assert_eq!(s.bucket, "bkt");
    assert_eq!(s.key, "a/b.txt");
    assert!(s.version_id.is_none());

    let s = parse_copy_source("bkt/key").unwrap();
    assert_eq!(s.bucket, "bkt");
    assert_eq!(s.key, "key");

    let s = parse_copy_source("/bkt/k%2Fpath?versionId=v1").unwrap();
    assert_eq!(s.key, "k/path");
    assert_eq!(s.version_id.as_deref(), Some("v1"));
}

#[test]
fn parse_range_ok_and_bad() {
    assert_eq!(parse_copy_source_range("bytes=0-9", 100).unwrap(), (0, 10));
    assert_eq!(parse_copy_source_range("bytes=5-5", 10).unwrap(), (5, 1));
    assert!(parse_copy_source_range("bytes=0-99", 50).is_err());
    assert!(parse_copy_source_range("bytes=10-5", 50).is_err());
    assert!(parse_copy_source_range("0-9", 50).is_err());
}

#[test]
fn get_range_reads_the_three_forms() {
    assert_eq!(
        parse_get_range("bytes=0-9"),
        Some(GetRange::From { first: 0, last: 9 })
    );
    assert_eq!(
        parse_get_range("bytes=40-"),
        Some(GetRange::From {
            first: 40,
            last: u64::MAX
        })
    );
    assert_eq!(
        parse_get_range("bytes=-20"),
        Some(GetRange::Suffix { length: 20 })
    );
    assert_eq!(
        parse_get_range("  bytes=1-2 "),
        Some(GetRange::From { first: 1, last: 2 })
    );
}

#[test]
fn get_range_ignores_what_it_cannot_read() {
    for raw in [
        "bytes=0-10,20-30",
        "bytes=",
        "bytes=-",
        "bytes=abc-def",
        "bytes=10-5",
        "items=0-9",
        "bytes=0-9,10",
        "0-9",
    ] {
        assert_eq!(parse_get_range(raw), None, "{raw}");
    }
}

#[test]
fn get_range_clips_to_the_object() {
    assert_eq!(
        satisfiable(GetRange::From { first: 0, last: 9 }, 100),
        Some((0, 9))
    );
    // Past the end of the object the last byte simply is the last byte.
    assert_eq!(
        satisfiable(
            GetRange::From {
                first: 90,
                last: u64::MAX
            },
            100
        ),
        Some((90, 99))
    );
    assert_eq!(
        satisfiable(
            GetRange::From {
                first: 0,
                last: u64::MAX
            },
            100
        ),
        Some((0, 99))
    );
    assert_eq!(satisfiable(GetRange::Suffix { length: 5 }, 3), Some((0, 2)));
    assert_eq!(satisfiable(GetRange::Suffix { length: 3 }, 3), Some((0, 2)));
}

#[test]
fn get_range_refuses_what_the_object_cannot_answer() {
    assert_eq!(
        satisfiable(
            GetRange::From {
                first: 100,
                last: u64::MAX
            },
            100
        ),
        None
    );
    assert_eq!(satisfiable(GetRange::From { first: 5, last: 9 }, 3), None);
    // A suffix of zero bytes asks for nothing at all.
    assert_eq!(satisfiable(GetRange::Suffix { length: 0 }, 3), None);
}

#[test]
fn every_range_on_an_empty_object_is_the_empty_object() {
    for range in [
        GetRange::From { first: 0, last: 9 },
        GetRange::From {
            first: 0,
            last: u64::MAX,
        },
        GetRange::From {
            first: 4,
            last: u64::MAX,
        },
        GetRange::Suffix { length: 0 },
        GetRange::Suffix { length: 9 },
    ] {
        assert_eq!(satisfiable(range, 0), Some((0, 0)), "{range:?}");
    }
}
