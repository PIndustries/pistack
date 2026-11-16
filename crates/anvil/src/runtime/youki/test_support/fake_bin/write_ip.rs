// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::shell_quote;
use super::write_exe;
use std::path::Path;

pub(crate) fn write_ip(path: &Path, log: &Path) {
    let quoted = shell_quote(log);
    write_exe(
        path,
        &format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" >> {quoted}\nprintf '\\n' >> {quoted}\nexit 0\n"
        ),
    );
}
