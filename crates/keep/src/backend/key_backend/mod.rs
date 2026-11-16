// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

mod from_env;
mod from_lookup;
// The enum keeps the module's own name, so the type lives in key_backend.rs
// rather than under a name that would drift from it.
#[allow(clippy::module_inception)]
mod key_backend;
mod load_existing;
mod load_or_create;
mod name;

pub use key_backend::KeyBackend;

#[cfg(test)]
mod from_lookup_tests;
#[cfg(test)]
mod from_lookup_token_file_tests;
#[cfg(test)]
mod from_lookup_vault_settings_tests;
#[cfg(test)]
mod load_existing_both_keys_tests;
#[cfg(test)]
mod load_existing_fails_closed_tests;
#[cfg(test)]
mod load_existing_refuses_a_software_store_tests;
#[cfg(test)]
mod load_or_create_vault_tests;
