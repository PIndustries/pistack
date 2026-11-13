// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `bench`: the `pi` command-line client and the web console.
//!
//! The CLI lives in [`cli`] as a library so it can be driven from tests
//! with captured output; `src/bin/pi.rs` is a four-line wrapper. The
//! `Bench` module mounts under `/console` and serves one HTML page that
//! signs in through `/identity/tokens` and calls the same API as `pi`.

pub mod cli;

use async_trait::async_trait;
use axum::{response::Html, routing::get, Router};
use pistack_types::{Module, ModuleContext, ModuleInfo};

const CONSOLE_HTML: &str = include_str!("console.html");

/// The console page. One document, no extra assets.
pub async fn console_page() -> Html<&'static str> {
    Html(CONSOLE_HTML)
}

#[derive(Default)]
pub struct Bench;

#[async_trait]
impl Module for Bench {
    fn info(&self) -> ModuleInfo {
        ModuleInfo {
            name: "bench",
            prefix: "console",
            summary: "The pi CLI and web console",
            version: pistack_types::BUILD,
        }
    }
    fn router(&self, _ctx: ModuleContext) -> Router {
        Router::new().route("/", get(console_page))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{to_bytes, Body};
    use axum::http::Request;
    use tower::ServiceExt;

    #[tokio::test]
    async fn console_page_is_the_portal() {
        let app = Router::new().route("/", get(console_page));
        let res = app
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), 200);
        let ctype = res
            .headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned();
        assert!(ctype.starts_with("text/html"));
        let body = to_bytes(res.into_body(), 2 << 20).await.unwrap();
        let text = String::from_utf8(body.to_vec()).unwrap();
        assert!(text.contains("/identity/tokens"));
        assert!(text.contains("/compute/machines"));
        assert!(text.contains("#login:not([hidden])"));
        assert!(text.contains("#shell:not([hidden])"));
        assert!(!text.contains("not built yet"));
        assert!(text.contains("#04060a"));
        assert!(text.contains("#34c759"));
        assert!(text.contains("#8df06a"));
        assert!(text.contains("#eafaef"));
        assert!(text.contains("Ports"));
        assert!(text.contains("Routers"));
        assert!(text.contains("Security groups"));
        assert!(text.contains("Projects"));
        assert!(text.contains("Users"));
        assert!(text.contains("Keys"));
        assert!(text.contains("Secrets"));
        assert!(text.contains("DNS"));
        assert!(text.contains("Load balancers"));
        assert!(text.contains("/network/ports"));
        assert!(text.contains("/identity/projects"));
        assert!(text.contains("/secrets/secrets"));
        assert!(text.contains("/dns/zones"));
        assert!(text.contains("/balancers/balancers"));
        assert!(text.contains("/security-groups/"));
        assert!(text.contains("/import"));
        assert!(text.contains("/volume/snapshots"));
        assert!(text.contains("pistack.io/auto"));
        assert!(text.contains("console/vnc"));
        assert!(text.contains(">Console<") || text.contains("\"Console\""));
        assert!(text.contains("A machine gets no floating IP on its own. If an admin has marked an external network for automatic floating IPs, the guest gets one from it when it has an address, and the auto DNS zone publishes its name."));
        assert!(text.contains("Name a network to boot on that project's bridge. A router on that network NATs the guest. The address on the port is the address the guest uses."));
        assert!(text.contains("security_groups"));
        assert!(text.contains(">Reboot<") || text.contains("\"Reboot\""));
        assert!(!text.contains("innerHTML"));
        assert!(text.contains("data-view=\"pods\""));
        assert!(text.contains("data-view=\"deployments\""));
        assert!(text.contains("data-view=\"statefulsets\""));
        assert!(text.contains("data-view=\"daemonsets\""));
        assert!(text.contains("data-view=\"autoscalers\""));
        assert!(text.contains("data-view=\"services\""));
        assert!(text.contains("data-view=\"jobs\""));
        assert!(text.contains("data-view=\"ingresses\""));
        assert!(text.contains("data-view=\"stacks\""));
        assert!(text.contains("data-view=\"shares\""));
        assert!(text.contains("data-view=\"alarms\""));
        assert!(text.contains("/loom/pods"));
        assert!(text.contains("/loom/deployments"));
        assert!(text.contains("/loom/statefulsets"));
        assert!(text.contains("/loom/daemonsets"));
        assert!(text.contains("/loom/autoscalers"));
        assert!(text.contains("/loom/services"));
        assert!(text.contains("/loom/jobs"));
        assert!(text.contains("/loom/ingresses"));
        assert!(text.contains("/blueprint/stacks"));
        assert!(text.contains("/apply"));
        assert!(text.contains("/shelf/shares"));
        assert!(text.contains("/gauge/alarms"));
        assert!(text.contains("Create pod"));
        assert!(text.contains("Create deployment"));
        assert!(text.contains("Create StatefulSet"));
        assert!(text.contains("Create DaemonSet"));
        assert!(text.contains("Create autoscaler"));
        assert!(text.contains("Create service"));
        assert!(text.contains("Create job"));
        assert!(text.contains("Create ingress"));
        assert!(text.contains("Create stack"));
        assert!(text.contains("Create share"));
        assert!(text.contains("Create alarm"));
        assert!(text.contains("\"Apply\""));
        assert!(text.contains("#7e9b86"));
        // The navigation is grouped and opens on the overview.
        assert!(text.contains("data-view=\"overview\" aria-current=\"page\""));
        for group in [
            "Compute",
            "Network",
            "Storage",
            "Workloads",
            "Access",
            "Monitoring",
        ] {
            assert!(text.contains(&format!("data-group=\"{group}\"")), "{group}");
        }
        assert_eq!(
            text.matches("<button type=\"button\" data-view=\"").count(),
            29
        );
        assert!(text.contains("Needs attention"));
        assert!(text.contains("Control plane"));
        assert!(text.contains("/identity/tokens/self/switch"));
        assert!(text.contains("Launch a machine"));
        assert!(text.contains("Recent activity"));
        assert!(text.contains("/gauge/samples"));
        assert!(!text.contains("10.66.66."));
        assert!(!text.contains("pistack0"));
        assert!(!text.to_ascii_lowercase().contains("ovn"));
    }
}
