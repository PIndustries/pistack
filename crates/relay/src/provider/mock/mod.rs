// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! In-memory provider: records every call, allocates VIPs from a private
//! range, serves every `spec.provider`. Tests assert on [`MockProvider::calls`].

mod call;
mod mock_provider;
mod state;

pub use call::Call;
pub use mock_provider::MockProvider;
