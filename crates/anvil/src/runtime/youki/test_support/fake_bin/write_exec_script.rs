// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::os::unix::fs::PermissionsExt;
use std::path::Path;

pub(crate) fn write_exec_script(path: &Path, log: &Path) {
    let quoted = format!("'{}'", log.display().to_string().replace('\'', "'\\''"));
    let body = format!(
        "#!/bin/sh\n\
         printf '%s\\n' \"$@\" >> {quoted}\n\
         printf '\\n' >> {quoted}\n\
         if [ \"$1\" = state ]; then\n\
           printf '%s\\n' '{{\"pid\":4242,\"status\":\"running\"}}'\n\
           exit 0\n\
         fi\n\
         if [ \"$1\" = exec ]; then\n\
           for arg in \"$@\"; do\n\
             if [ \"$arg\" = __sleep__ ]; then\n\
               sleep 30\n\
               exit 0\n\
             fi\n\
             if [ \"$arg\" = __flood__ ]; then\n\
               i=0\n\
               while [ \"$i\" -lt 200 ]; do\n\
                 printf x\n\
                 i=$((i + 1))\n\
               done\n\
               i=0\n\
               while [ \"$i\" -lt 80 ]; do\n\
                 printf y >&2\n\
                 i=$((i + 1))\n\
               done\n\
               exit 0\n\
             fi\n\
           done\n\
           printf 'out-line\\n'\n\
           printf 'err-line\\n' >&2\n\
           exit 7\n\
         fi\n\
         exit 0\n"
    );
    std::fs::write(path, body).unwrap();
    let mut perms = std::fs::metadata(path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms).unwrap();
}
