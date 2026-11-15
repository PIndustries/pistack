// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::volumes::slab_source_error;
use crate::runtime::youki::volumes::SlabSource;

#[test]
fn slab_source_errors_name_the_volume() {
    let missing = slab_source_error("disk", "/var/lib/pistack/vol", SlabSource::Missing);
    assert!(missing.message.contains("disk"), "{missing}");
    assert!(
        missing.message.contains("/var/lib/pistack/vol"),
        "{missing}"
    );
    assert!(missing.message.contains("does not exist"), "{missing}");

    let file = slab_source_error("disk", "/var/lib/pistack/disk", SlabSource::File);
    assert!(file.message.contains("disk"), "{file}");
    assert!(file.message.contains("/var/lib/pistack/disk"), "{file}");
    assert!(file.message.contains("is a file"), "{file}");
    assert!(file.message.contains("not a mounted directory"), "{file}");

    let block = slab_source_error("data", "/dev/rbd0", SlabSource::Block);
    assert!(block.message.contains("data"), "{block}");
    assert!(block.message.contains("/dev/rbd0"), "{block}");
    assert!(block.message.contains("block device"), "{block}");
    assert!(block.message.contains("not a mounted directory"), "{block}");
}
