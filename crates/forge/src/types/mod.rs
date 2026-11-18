// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The resources `forge` owns: `Host`, `Size` and `Machine`.
//!
//! All three are `Resource<Spec, Status>` envelopes from `pistack-types`.
//! `spec` is written by handlers (or, for hosts, by the agent report that
//! the transport delivers); `status` is written only by the reconciler.

macro_rules! resource_kind {
    ($(#[$doc:meta])* $name:ident, $spec:ty, $status:ty, $kind:expr) => {
        $(#[$doc])*
        #[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
        #[serde(transparent)]
        pub struct $name(pub Resource<$spec, $status>);

        impl Deref for $name {
            type Target = Resource<$spec, $status>;
            fn deref(&self) -> &Self::Target {
                &self.0
            }
        }
        impl DerefMut for $name {
            fn deref_mut(&mut self) -> &mut Self::Target {
                &mut self.0
            }
        }
        impl From<Resource<$spec, $status>> for $name {
            fn from(r: Resource<$spec, $status>) -> Self {
                Self(r)
            }
        }
        impl $name {
            pub fn into_inner(self) -> Resource<$spec, $status> {
                self.0
            }
        }
        impl Kind for $name {
            const KIND: &'static str = $kind;
            fn meta(&self) -> &Meta {
                &self.0.meta
            }
            fn meta_mut(&mut self) -> &mut Meta {
                &mut self.0.meta
            }
        }
    };
}

mod default_sizes;
mod delete_annotation;
mod delete_requested;
mod desired_power;
mod disk_label;
mod disk_shared;
mod evacuated_from_annotation;
mod host;
mod host_spec;
mod host_status;
mod keypair;
mod keypair_kind;
mod keypair_spec;
mod keypair_status;
mod machine;
mod machine_disks_are_shared;
mod machine_kind;
mod machine_spec;
mod machine_status;
mod new_host;
mod new_keypair;
mod new_machine;
mod new_size;
mod node_pin_annotation;
mod power_control;
mod power_state;
mod rebuild_annotation;
mod rebuild_requested;
mod size;
mod size_kind;
mod size_spec;
mod size_status;
mod system_label_prefix;
mod underlay_ip_ok;
mod usable_underlay;
mod wait_volume_annotation;
mod zone_label;
pub use default_sizes::default_sizes;
pub use delete_annotation::DELETE_ANNOTATION;
pub use delete_requested::delete_requested;
pub use desired_power::DesiredPower;
pub use disk_label::DISK_LABEL;
pub use disk_shared::DISK_SHARED;
pub use evacuated_from_annotation::EVACUATED_FROM_ANNOTATION;
pub use host::Host;
pub use host_spec::HostSpec;
pub use host_status::HostStatus;
pub use keypair::Keypair;
pub use keypair_kind::KEYPAIR_KIND;
pub use keypair_spec::KeypairSpec;
pub use keypair_status::KeypairStatus;
pub use machine::Machine;
pub use machine_disks_are_shared::machine_disks_are_shared;
pub use machine_kind::MachineKind;
pub use machine_spec::MachineSpec;
pub use machine_status::MachineStatus;
pub use new_host::new_host;
pub use new_keypair::new_keypair;
pub use new_machine::new_machine;
pub use new_size::new_size;
pub use node_pin_annotation::NODE_PIN_ANNOTATION;
pub use power_control::PowerControl;
pub use power_state::PowerState;
pub use rebuild_annotation::REBUILD_ANNOTATION;
pub use rebuild_requested::rebuild_requested;
pub use size::Size;
pub use size_kind::SIZE_KIND;
pub use size_spec::SizeSpec;
pub use size_status::SizeStatus;
pub use system_label_prefix::SYSTEM_LABEL_PREFIX;
pub(crate) use usable_underlay::usable_underlay;
pub use wait_volume_annotation::WAIT_VOLUME_ANNOTATION;
pub use zone_label::ZONE_LABEL;
