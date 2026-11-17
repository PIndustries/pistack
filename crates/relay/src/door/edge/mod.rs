// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The edge path: find the edge host, queue one publish for the listener,
//! and start no process.

mod apply_edge;
mod edge_node;
mod edge_publish;
mod edge_published;
mod host_spec_view;
mod host_status_view;

pub(crate) use apply_edge::apply_edge;
pub(crate) use edge_published::edge_published;
#[cfg(test)]
pub(crate) use host_spec_view::HostSpecView;
#[cfg(test)]
pub(crate) use host_status_view::HostStatusView;
