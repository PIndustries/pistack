// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use pistack_rpc::proto::ProbeReport;
pub(in crate::controllers::pod) fn probe_report<'a>(
    reports: &'a [ProbeReport],
    container: &str,
    kind: &str,
) -> Option<&'a ProbeReport> {
    reports
        .iter()
        .find(|r| r.container == container && r.kind == kind)
}
#[cfg(test)]
#[path = "probe_report_tests.rs"]
mod probe_report_tests;
