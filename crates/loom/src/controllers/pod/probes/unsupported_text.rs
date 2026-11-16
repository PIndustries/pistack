// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use pistack_rpc::proto::ProbeReport;
pub(in crate::controllers::pod) fn unsupported_text(report: Option<&ProbeReport>) -> String {
    report
        .map(|r| r.message.trim())
        .filter(|message| !message.is_empty())
        .unwrap_or("probe is not supported")
        .to_owned()
}
#[cfg(test)]
#[path = "unsupported_text_tests.rs"]
mod unsupported_text_tests;
