// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Automatic machine records.
//!
//! A zone labelled `pistack.io/auto=true` gets an `A` (and, when the machine
//! has IPv6, an `AAAA`) set `<machine-name>.<zone>` for every machine in the
//! zone's project. The address is the machine's floating IP when one is
//! associated with any of its ports, otherwise its fixed addresses as
//! reported in `status.addresses`. Sets beacon writes carry
//! `pistack.io/managed-by=beacon-auto`; sets with that label whose machine
//! has gone are removed. A pass is idempotent, so it runs on every `mach`,
//! `fip` and `port` event and on a timer with no harm.
//!
//! `forge` and `fabric` own machines, ports and floating IPs; beacon reads
//! them through the store's JSON wire shape with narrow views below, so it
//! does not depend on those crates and cannot write to their resources.

mod auto_zones;
mod floating_ip_spec_view;
mod floating_ip_view;
mod machine_addresses;
mod machine_status_view;
mod machine_view;
mod owner;
mod port_spec_view;
mod port_view;
mod resync;
mod run_pass;
mod scan;
mod start;
mod sync_all;
mod sync_zone;
#[cfg(test)]
mod tests;
mod triggers;
pub use owner::OWNER;
pub use start::start;
pub use sync_all::sync_all;
