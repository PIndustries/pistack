// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

#[cfg(test)]
mod tests {
    use super::super::test_support::*;
    use super::super::{gateway_candidates, HostRecord, MAX_GATEWAY_CHASSIS};
    use pistack_store::Store;
    use pistack_types::Phase;
    #[test]
    fn at_most_five_chassis_carry_a_gateway() {
        let store = Store::in_memory().unwrap();
        let hosts = store.collection::<HostRecord>();
        for i in (0..8).rev() {
            hosts
                .create(host_record(
                    &format!("node-{i}"),
                    Phase::Ready,
                    Some((true, "physnet1:br-ex")),
                ))
                .unwrap();
        }
        let got = gateway_candidates(&store).unwrap();
        assert_eq!(got.len(), MAX_GATEWAY_CHASSIS);
        assert_eq!(got[0], "node-0");
        assert_eq!(got[4], "node-4");
    }
}
