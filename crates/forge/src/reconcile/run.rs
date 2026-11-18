// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::ModuleContext;

use super::reconciler::Reconciler;

impl Reconciler {
    /// Run only while this node is the cluster leader. Idle on followers;
    /// restart on promotion; stop on demotion or shutdown.
    pub async fn run(self, ctx: ModuleContext) {
        let this = std::sync::Arc::new(self);
        let leadership = ctx.leadership.clone();
        leadership
            .run_while_leader(ctx.shutdown.clone(), move |active| {
                let this = this.clone();
                async move { this.run_loop(active).await }
            })
            .await;
    }
}
