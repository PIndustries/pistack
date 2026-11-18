// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Host maintenance: getting machines off a host.
//!
//! [`drain`] is for a host that is up. It live-migrates what can be
//! live-migrated and reports the rest. It never stops a machine.
//!
//! [`evacuate`] is for a host that is gone. Nothing can be copied off it,
//! so a machine is handed back to the scheduler and starts somewhere else
//! from its image. Its root disk, which was a file on the dead host, is
//! not there. Volumes on shared storage are plugged back in by slab;
//! a machine with a volume that lived on the dead host is left alone.
//! The agent of a host that returns is told to delete the copies it still
//! holds; see `Registrar::delete_stale_copy`.

mod bmc_fencer;
mod can_live_migrate;
mod disk_holds_machine;
mod drain;
mod drain_report;
mod evacuate;
mod evacuate_options;
mod evacuate_report;
mod evacuated;
mod evacuation;
mod fence_failed;
mod fence_poll;
mod fence_timeout;
mod fencer;
mod kind_word;
mod left;
mod live_migration_driver;
mod migrating_to;
mod moving;
mod needs_proof;
mod no_destination;
mod no_status;
mod phase_word;
mod proof;
mod prove_off;
mod record_proof;
mod shared_backends;
mod size_lookup;
mod still_reporting;
mod volume;
mod volume_names;
mod volume_spec_view;
mod volume_status_view;
mod volume_type;
mod volume_type_spec_view;
pub use bmc_fencer::BmcFencer;
pub use can_live_migrate::can_live_migrate;
pub use disk_holds_machine::disk_holds_machine;
pub use drain::drain;
pub use drain_report::DrainReport;
pub use evacuate::evacuate;
pub use evacuate_options::EvacuateOptions;
pub use evacuate_report::EvacuateReport;
pub use evacuated::Evacuated;
pub use evacuation::Evacuation;
pub use fence_timeout::FENCE_TIMEOUT;
pub use fencer::Fencer;
pub use left::Left;
pub use live_migration_driver::LIVE_MIGRATION_DRIVER;
pub use moving::Moving;
pub(crate) use phase_word::phase_word;
pub use proof::Proof;
