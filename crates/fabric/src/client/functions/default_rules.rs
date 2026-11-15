// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use crate::types::*;
use pistack_types::Id;
/// Rules of the template "default" group: all egress out, ingress only from
/// the group itself. `me` is the template's own id.
pub fn default_rules(me: &Id) -> Vec<Rule> {
    vec![
        Rule::new(Direction::Egress, EtherType::IPv4),
        Rule::new(Direction::Egress, EtherType::IPv6),
        Rule::new(Direction::Ingress, EtherType::IPv4).remote(Remote::Group(me.clone())),
        Rule::new(Direction::Ingress, EtherType::IPv6).remote(Remote::Group(me.clone())),
    ]
}
