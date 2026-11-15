// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::probe::exec::validate_exec_argv::validate_exec_argv;

#[test]
fn exec_argv_rejects_an_empty_command_and_a_nul_byte() {
    assert_eq!(
        validate_exec_argv(&[]).unwrap_err(),
        "exec command is empty"
    );
    assert_eq!(
        validate_exec_argv(&[String::new()]).unwrap_err(),
        "exec command is empty"
    );
    assert_eq!(
        validate_exec_argv(&["/bin/echo".into(), String::new()]).unwrap_err(),
        "exec command is empty"
    );
    let nul = format!("a{}b", '\0');
    assert_eq!(
        validate_exec_argv(&["/bin/echo".into(), nul]).unwrap_err(),
        "exec command contains a NUL byte"
    );
    assert!(validate_exec_argv(&["/bin/true".into()]).is_ok());
}
