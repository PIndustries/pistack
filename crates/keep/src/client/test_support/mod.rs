// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The tests of this module and what they share.

mod a_certificate_is_renewed_for_a_new_key_and_the_old_one_is_marked_tests;
mod a_listener_certificate_with_no_record_here_is_replaced_by_one_like_it_tests;
mod a_revoked_or_deleted_certificate_is_not_renewed_tests;
mod a_server_certificate_keeps_its_names_when_renewed_tests;
pub(in crate::client::test_support) mod cert_spec;
mod certificate_metadata_has_the_secret_limits_tests;
mod certificates_per_project_limit_is_exact_and_keys_count_there_tests;
pub(in crate::client::test_support) mod client;
mod client_round_trips_secrets_through_vault_transit_tests;
mod concurrent_creates_cannot_overshoot_the_limit_tests;
pub(in crate::client::test_support) mod denied;
mod gate_project_quota_overrides_the_default_and_records_usage_tests;
mod issue_pki_persists_resources_and_keys_decrypt_tests;
pub(in crate::client::test_support) mod limited;
mod metadata_limits_apply_on_create_and_update_tests;
pub(in crate::client::test_support) mod mk;
mod payload_limit_is_inclusive_on_create_and_rotate_tests;
mod reserved_role_label_cannot_be_forged_or_stripped_tests;
mod secret_lifecycle_through_client_tests;
mod secrets_per_project_default_limit_is_exact_tests;
pub(in crate::client::test_support) mod serial_of;
