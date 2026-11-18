// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use axum::{
    routing::{get, post, put},
    Router,
};

use super::app_state::AppState;
use super::clear_host_power::clear_host_power;
use super::console::console;
use super::console_vnc::console_vnc;
use super::create_keypair::create_keypair;
use super::create_machine::create_machine;
use super::create_size::create_size;
use super::delete_keypair::delete_keypair;
use super::delete_machine::delete_machine;
use super::delete_size::delete_size;
use super::disable_host::disable_host;
use super::drain_host::drain_host;
use super::enable_host::enable_host;
use super::evacuate_host::evacuate_host;
use super::get_host::get_host;
use super::get_host_power::get_host_power;
use super::get_keypair::get_keypair;
use super::get_machine::get_machine;
use super::get_size::get_size;
use super::host_stats::host_stats;
use super::index::index;
use super::list_hosts::list_hosts;
use super::list_keypairs::list_keypairs;
use super::list_machines::list_machines;
use super::list_sizes::list_sizes;
use super::machine_stats::machine_stats;
use super::migrate_machine::migrate_machine;
use super::reboot_machine::reboot_machine;
use super::rebuild_machine::rebuild_machine;
use super::resize_machine::resize_machine;
use super::set_host_labels::set_host_labels;
use super::set_host_power::set_host_power;
use super::start_machine::start_machine;
use super::stop_machine::stop_machine;
use super::update_machine::update_machine;
use super::update_size::update_size;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/machines", get(list_machines).post(create_machine))
        .route(
            "/machines/{id}",
            get(get_machine).put(update_machine).delete(delete_machine),
        )
        .route("/machines/{id}/start", post(start_machine))
        .route("/machines/{id}/stop", post(stop_machine))
        .route("/machines/{id}/reboot", post(reboot_machine))
        .route("/machines/{id}/rebuild", post(rebuild_machine))
        .route("/machines/{id}/resize", post(resize_machine))
        .route("/machines/{id}/migrate", post(migrate_machine))
        .route("/machines/{id}/stats", get(machine_stats))
        .route("/machines/{id}/console", get(console))
        .route("/machines/{id}/console/vnc", get(console_vnc))
        .route("/hosts", get(list_hosts))
        .route("/hosts/{id}", get(get_host))
        .route("/hosts/{id}/stats", get(host_stats))
        .route("/hosts/{id}/labels", put(set_host_labels))
        .route("/hosts/{id}/disable", post(disable_host))
        .route("/hosts/{id}/enable", post(enable_host))
        .route("/hosts/{id}/drain", post(drain_host))
        .route("/hosts/{id}/evacuate", post(evacuate_host))
        .route(
            "/hosts/{id}/power",
            get(get_host_power)
                .put(set_host_power)
                .delete(clear_host_power),
        )
        .route("/sizes", get(list_sizes).post(create_size))
        .route(
            "/sizes/{id}",
            get(get_size).put(update_size).delete(delete_size),
        )
        .route("/keypairs", get(list_keypairs).post(create_keypair))
        .route("/keypairs/{id}", get(get_keypair).delete(delete_keypair))
        .with_state(state)
}
