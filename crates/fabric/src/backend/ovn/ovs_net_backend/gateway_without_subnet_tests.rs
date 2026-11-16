// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

#[cfg(test)]
mod tests {
    use super::super::super::test_support::*;
    use crate::backend::NetBackend;
    #[tokio::test]
    async fn a_gateway_without_its_subnet_waits() {
        let (_db, b, mut view) = routed().await;
        view.gateway_subnet = None;
        let err = b.ensure_router(&view).await.unwrap_err();
        assert!(err.message.contains("has no subnet"));
    }
}
