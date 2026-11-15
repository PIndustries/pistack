// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::os::unix::fs::PermissionsExt;
use std::path::Path;

pub(crate) fn write_sudo(path: &Path, log: &Path) {
    let quoted = format!("'{}'", log.display().to_string().replace('\'', "'\\''"));
    let body = format!(
        "#!/bin/sh\nprintf '%s\\n' \"$@\" >> {quoted}\nprintf '\\n' >> {quoted}\nif [ \"$1\" = -n ]; then shift; fi\nexec \"$@\"\n"
    );
    std::fs::write(path, body).unwrap();
    let mut perms = std::fs::metadata(path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms).unwrap();
}
