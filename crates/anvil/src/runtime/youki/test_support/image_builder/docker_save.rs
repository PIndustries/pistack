// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::gzip_bytes;
use super::tar_file;

pub(crate) fn docker_save() -> Vec<u8> {
    let layer1 = tar_file(&[("keep", b"yes"), ("gone", b"old"), ("sub/old", b"x")]);
    let layer2 = gzip_bytes(&tar_file(&[
        (".wh.gone", b""),
        ("sub/.wh..wh..opq", b""),
        ("sub/new", b"n"),
    ]));
    let config = br#"{"config":{"Entrypoint":["/bin/app"],"Cmd":["serve"],"Env":["APP_MODE=image"],"WorkingDir":"/app"}}"#;
    let manifest = br#"[{"Config":"config.json","Layers":["layer1.tar","layer2.tar.gz"]}]"#;
    tar_file(&[
        ("manifest.json", manifest),
        ("config.json", config),
        ("layer1.tar", &layer1),
        ("layer2.tar.gz", &layer2),
    ])
}
