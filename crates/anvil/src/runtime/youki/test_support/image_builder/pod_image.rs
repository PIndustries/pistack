// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::path::Path;

pub(crate) fn pod_image(tmp: &Path) -> String {
    let image = tmp.join("image");
    std::fs::create_dir_all(image.join("bin")).unwrap();
    std::fs::write(image.join("marker"), b"keep").unwrap();
    image.to_str().unwrap().to_owned()
}
