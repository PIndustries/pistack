// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The DNS resource model: `Zone` and `RecordSet`.
//!
//! A zone is an authoritative apex (`example.cloud.`) with the SOA and NS
//! data synthesised from its spec; a record set is one owner name and one
//! type inside a zone with one or more RDATA strings. `spec` is the user's,
//! `status` is the reconciler's: it carries the zone serial, which the
//! reconciler bumps whenever the zone's content changes.
//!
//! Names are stored as absolute, lower-case FQDNs with a trailing dot.

macro_rules! resource {
    ($(#[$doc:meta])* $t:ident, $spec:ty, $status:ty, $k:expr) => {
        $(#[$doc])*
        #[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
        pub struct $t {
            #[serde(flatten)]
            pub meta: Meta,
            pub spec: $spec,
            pub status: $status,
        }
        impl Kind for $t {
            const KIND: &'static str = $k;
            fn meta(&self) -> &Meta {
                &self.meta
            }
            fn meta_mut(&mut self) -> &mut Meta {
                &mut self.meta
            }
        }
        impl From<Resource<$spec, $status>> for $t {
            fn from(r: Resource<$spec, $status>) -> Self {
                Self { meta: r.meta, spec: r.spec, status: r.status }
            }
        }
        impl From<$t> for Resource<$spec, $status> {
            fn from(r: $t) -> Self {
                Self { meta: r.meta, spec: r.spec, status: r.status }
            }
        }
    };
}

mod auto_label;
mod default_email;
mod default_ttl;
mod export_zone;
mod in_zone;
mod kinds;
mod managed_label;
mod max_also_notify;
mod normalize_name;
mod notify_addr;
mod owner_in_zone;
mod record_set;
mod record_set_spec;
mod record_set_status;
mod record_type;
#[cfg(test)]
mod tests;
mod validate_rdata;
mod validate_recordset;
mod validate_zone;
mod zone;
mod zone_kind;
mod zone_spec;
mod zone_status;
pub use auto_label::AUTO_LABEL;
pub use default_ttl::default_ttl;
pub use export_zone::export_zone;
pub use in_zone::in_zone;
pub use kinds::KINDS;
pub use managed_label::MANAGED_LABEL;
pub use max_also_notify::MAX_ALSO_NOTIFY;
pub use normalize_name::normalize_name;
pub use owner_in_zone::owner_in_zone;
pub use record_set::RecordSet;
pub use record_set_spec::RecordSetSpec;
pub use record_set_status::RecordSetStatus;
pub use record_type::RecordType;
pub use validate_rdata::validate_rdata;
pub use validate_recordset::validate_recordset;
pub use validate_zone::validate_zone;
pub use zone::Zone;
pub use zone_kind::ZoneKind;
pub use zone_spec::ZoneSpec;
pub use zone_status::ZoneStatus;
