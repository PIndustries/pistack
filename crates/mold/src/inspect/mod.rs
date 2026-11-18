// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Look inside an uploaded disk image.
//!
//! With `qemu-img` on the server: `qemu-img info --output=json -f <fmt>`
//! (the declared format is forced so qemu never probes; probing is how a
//! raw image that *looks like* qcow2 gets a backing file pointed at a
//! host path). Without it: read the first 512 bytes and parse the qcow2
//! header ourselves. Either way an image that names a backing file or an
//! external data file is rejected: it would make the hypervisor open an
//! arbitrary file on the compute host.

mod check_declared;
mod detect_magic;
mod detected_format;
mod inspect_type;
mod inspection;
mod parse_qemu_img_info;
mod qcow2_magic;
mod qemu_format_data;
mod qemu_format_specific;
mod qemu_img_available;
mod qemu_info;
mod read_head;
mod run_qemu_img;
#[cfg(test)]
mod tests;
pub use check_declared::check_declared;
pub use detect_magic::detect_magic;
pub use detected_format::DetectedFormat;
pub use inspect_type::inspect;
pub use inspection::Inspection;
pub use parse_qemu_img_info::parse_qemu_img_info;
pub use qcow2_magic::QCOW2_MAGIC;
pub use qemu_img_available::qemu_img_available;
