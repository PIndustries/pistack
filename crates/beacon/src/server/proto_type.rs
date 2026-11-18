// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::RecordType as Rt;
use hickory_proto::rr::RecordType;

pub(crate) fn proto_type(k: Rt) -> RecordType {
    match k {
        Rt::A => RecordType::A,
        Rt::Aaaa => RecordType::AAAA,
        Rt::Cname => RecordType::CNAME,
        Rt::Mx => RecordType::MX,
        Rt::Txt => RecordType::TXT,
        Rt::Srv => RecordType::SRV,
        Rt::Ns => RecordType::NS,
        Rt::Ptr => RecordType::PTR,
    }
}
