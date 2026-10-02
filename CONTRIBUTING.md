# Contributing

## Sign-off

Every commit carries a Developer Certificate of Origin sign-off (`git commit -s`), the same rule as the Linux kernel. By signing off you certify https://developercertificate.org/. CI rejects commits without it.

## Layout

- `bin/pistack` — the binary. Thin: parses flags, builds the gateway, mounts modules.
- `crates/pistack-*` — shared crates. No module code here.
- `crates/<module>` — one crate per module. A module depends on `pistack-types` and `pistack-store`, and on other modules only through their public client types, never their internals.

## Rules of the house

- A module owns its resources. Nobody else writes to them.
- `spec` is the user's; `status` is the reconciler's. Handlers never write `status`.
- Every handler returns `ApiResult<T>`; errors are `ApiError`, never strings.
- Every resource kind has a test that creates, lists, updates with a stale revision (expects conflict) and deletes.
- No `unwrap()` outside tests. No `println!`; use `tracing`.
- Rust 2021, `cargo fmt`, `cargo clippy -D warnings` clean.
- Plain nouns for API paths and CLI subcommands (`/machines`, `pi machine`), workshop names for modules.
