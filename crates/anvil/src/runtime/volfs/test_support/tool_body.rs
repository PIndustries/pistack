// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::shell_quote::shell_quote;
use std::path::Path;

pub(in crate::runtime::volfs) fn tool_body(log: &Path, behavior: &str) -> String {
    format!(
        "#!/bin/sh\nbase=$(basename \"$0\")\nprintf '%s\\n' \"$base\" \"$@\" >> {}\nprintf '\\n' >> {}\n{behavior}",
        shell_quote(log),
        shell_quote(log)
    )
}
