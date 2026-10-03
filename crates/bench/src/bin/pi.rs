// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `pi` — the PiStack command-line client. All logic lives in `bench::cli`.

use std::io::IsTerminal;

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let stderr = std::io::stderr();
    let tty = stdin.is_terminal();
    let mut io = bench::cli::Io {
        stdin: &mut stdin.lock(),
        stdout: &mut stdout.lock(),
        stderr: &mut stderr.lock(),
        tty,
    };
    let code = match bench::cli::run(args, &mut io).await {
        Ok(code) => code,
        // A closed pipe (`pi ... | head`) is not an error worth reporting.
        Err(e)
            if e.downcast_ref::<std::io::Error>()
                .is_some_and(|io| io.kind() == std::io::ErrorKind::BrokenPipe) =>
        {
            0
        }
        Err(e) => {
            eprintln!("error: {e}");
            1
        }
    };
    std::process::exit(code);
}
