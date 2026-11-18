// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Resource kinds owned by `mold`: images and image members.

macro_rules! resource_kind {
    ($(#[$doc:meta])* $name:ident, $spec:ty, $status:ty, $k:expr) => {
        $(#[$doc])*
        #[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
        pub struct $name {
            #[serde(flatten)]
            pub meta: Meta,
            pub spec: $spec,
            pub status: $status,
        }
        impl Kind for $name {
            const KIND: &'static str = $k;
            fn meta(&self) -> &Meta {
                &self.meta
            }
            fn meta_mut(&mut self) -> &mut Meta {
                &mut self.meta
            }
        }
        impl From<$name> for Resource<$spec, $status> {
            fn from(r: $name) -> Self {
                Resource { meta: r.meta, spec: r.spec, status: r.status }
            }
        }
        impl From<Resource<$spec, $status>> for $name {
            fn from(r: Resource<$spec, $status>) -> Self {
                Self { meta: r.meta, spec: r.spec, status: r.status }
            }
        }
    };
}

mod default_arch;
mod image;
mod image_format;
mod image_member;
mod image_member_spec;
mod image_member_status;
mod image_spec;
mod image_status;
mod kind_image;
mod kind_image_member;
mod set_condition;
mod visibility;
pub use image::Image;
pub use image_format::ImageFormat;
pub use image_member::ImageMember;
pub use image_member_spec::ImageMemberSpec;
pub use image_member_status::ImageMemberStatus;
pub use image_spec::ImageSpec;
pub use image_status::ImageStatus;
pub use kind_image::KIND_IMAGE;
pub use kind_image_member::KIND_IMAGE_MEMBER;
pub use set_condition::set_condition;
pub use visibility::Visibility;
