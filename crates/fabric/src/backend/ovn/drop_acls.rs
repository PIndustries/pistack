// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::pub_consts::METADATA_ADDRESS;
use super::{
    acl::Acl,
    consts::{DROP_GROUP, DROP_PRIORITY, RULE_PRIORITY},
};

pub(in crate::backend::ovn) fn drop_acls() -> Vec<Acl> {
    let g = DROP_GROUP;
    let acl = |direction, priority, matches: String, action| Acl {
        direction,
        priority,
        matches,
        action,
        rule: None,
    };
    vec![
        acl(
            "from-lport",
            DROP_PRIORITY,
            format!("inport == @{g} && ip"),
            "drop",
        ),
        acl(
            "to-lport",
            DROP_PRIORITY,
            format!("outport == @{g} && ip"),
            "drop",
        ),
        acl(
            "from-lport",
            RULE_PRIORITY,
            format!("inport == @{g} && udp && udp.src == 68 && udp.dst == 67"),
            "allow",
        ),
        acl(
            "from-lport",
            RULE_PRIORITY,
            format!("inport == @{g} && udp && udp.src == 546 && udp.dst == 547"),
            "allow",
        ),
        acl(
            "from-lport",
            RULE_PRIORITY,
            format!(
                "inport == @{g} && ip4 && ip4.dst == {METADATA_ADDRESS} && tcp && tcp.dst == 80"
            ),
            "allow-related",
        ),
    ]
}
