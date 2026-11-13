// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Background removal of dead join tokens, and a warning about
//! certificates nobody is renewing.
//!
//! An agent or a server renews its own certificate once two thirds of its
//! lifetime have passed ([`crate::ca::renew_at`]). The control plane only
//! watches: [`certificates_not_renewing`] finds the ones that are past that
//! point by [`RENEWAL_GRACE`] with no successor issued, and the leader logs
//! each of them every [`WARN_EVERY`] until it is renewed, revoked or
//! deleted, or has been expired for [`FORGET_AFTER`].
//!
//! A join token stops being usable when it expires or when its last use
//! is spent. Its record stays for [`KeepConfig::join_token_grace_secs`]
//! (default 7 days) so an admin can still see which token enrolled what,
//! and is then deleted by [`sweep_join_tokens`]. Live tokens are never
//! touched, and a dead token cannot come back to life (expiry and
//! `max_uses` do not change), so a sweep cannot race a redeem into
//! deleting a usable token.
//!
//! The sweep is a plain function over the store and an explicit `now` so
//! it is testable. [`Keep`](crate::Keep) runs [`run`] under
//! [`pistack_types::Leadership::run_while_leader`], like silo's multipart
//! sweep: only the leader writes, and its deletes replicate. It sweeps once
//! on promotion (startup on a sole server) and then every
//! [`sweep_interval`].
//!
//! [`KeepConfig::join_token_grace_secs`]: crate::KeepConfig::join_token_grace_secs

use chrono::{DateTime, Utc};
use pistack_store::{Store, StoreError};
use pistack_types::CancellationToken;
use std::time::Duration;

use crate::client::managed_cert_role;
use crate::types::{join_token_spent, Certificate, JoinToken};

/// Longest gap between sweeps.
pub const MAX_SWEEP_INTERVAL: Duration = Duration::from_secs(60 * 60);
/// Shortest gap between sweeps, even with a grace of zero.
pub const MIN_SWEEP_INTERVAL: Duration = Duration::from_secs(60);

/// What one sweep did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SweepReport {
    /// Join tokens looked at.
    pub scanned: usize,
    /// Dead tokens past the grace period that were deleted.
    pub deleted: usize,
    /// Deletes that failed (logged; retried next sweep).
    pub failed: usize,
}

/// Gap between sweeps for `grace`: the grace period, between
/// [`MIN_SWEEP_INTERVAL`] and [`MAX_SWEEP_INTERVAL`].
pub fn sweep_interval(grace: Duration) -> Duration {
    grace.clamp(MIN_SWEEP_INTERVAL, MAX_SWEEP_INTERVAL)
}

/// When `t` stopped being usable, or `None` while it is live: the earlier
/// of its expiry and its last use, if that use spent it.
pub fn dead_since(t: &JoinToken, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    let expired = (t.spec.expires_at <= now).then_some(t.spec.expires_at);
    let spent = join_token_spent(t).then(|| t.status.last_used.unwrap_or(t.meta.created_at));
    match (expired, spent) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}

/// Delete every join token that has been dead for at least `grace` at `now`.
pub fn sweep_join_tokens(store: &Store, now: DateTime<Utc>, grace: Duration) -> SweepReport {
    let mut report = SweepReport::default();
    let col = store.collection::<JoinToken>();
    let tokens = match col.list(None) {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!(error = %e, "keep sweep: cannot list join tokens");
            report.failed += 1;
            return report;
        }
    };
    let grace = chrono::Duration::from_std(grace).unwrap_or(chrono::Duration::MAX);
    for t in tokens {
        report.scanned += 1;
        let Some(dead) = dead_since(&t, now) else {
            continue;
        };
        if dead.checked_add_signed(grace).is_none_or(|due| due > now) {
            continue;
        }
        match col.delete(&t.meta.id) {
            Ok(_) => {
                report.deleted += 1;
                tracing::debug!(token = %t.meta.id, name = %t.meta.name, "keep sweep: deleted join token");
            }
            // Someone deleted it first; nothing left to do.
            Err(StoreError::NotFound(_)) => {}
            Err(StoreError::NotLeader) => {
                tracing::debug!(token = %t.meta.id, "keep sweep: not the leader");
            }
            Err(e) => {
                report.failed += 1;
                tracing::warn!(token = %t.meta.id, error = %e, "keep sweep: delete failed");
            }
        }
    }
    report
}

/// How long past its renewal point a certificate may be before it is
/// reported: a holder that is up renews within the hour.
pub const RENEWAL_GRACE: chrono::Duration = chrono::Duration::days(1);
/// A certificate expired this long is no longer reported.
pub const FORGET_AFTER: chrono::Duration = chrono::Duration::days(30);
/// Gap between two reports of the same certificates.
pub const WARN_EVERY: Duration = Duration::from_secs(6 * 60 * 60);

/// A certificate of the cloud's own that should have been renewed by now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NotRenewing {
    /// Resource name of the certificate.
    pub name: String,
    /// `agent` or `server`.
    pub role: &'static str,
    /// The node name for an agent.
    pub common_name: String,
    pub serial: u64,
    pub not_after: DateTime<Utc>,
    pub expired: bool,
}

/// The agent and server certificates that are past their renewal point at
/// `now` and that nothing has replaced. Revoked ones, renewed ones and
/// certificates a project asked for are not listed.
pub fn certificates_not_renewing(store: &Store, now: DateTime<Utc>) -> Vec<NotRenewing> {
    let certs = match store.collection::<Certificate>().list(None) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(error = %e, "keep sweep: cannot list certificates");
            return Vec::new();
        }
    };
    let mut out = Vec::new();
    for c in &certs {
        let Some(role) = managed_cert_role(c) else {
            continue;
        };
        if c.status.revoked || c.status.renewed_by.is_some() {
            continue;
        }
        let (Some(not_before), Some(not_after)) = (c.status.not_before, c.status.not_after) else {
            continue;
        };
        let renew_at = crate::ca::renew_at(not_before, not_after);
        // A short-lived certificate gets a shorter grace, never past half
        // of what is left after its renewal point.
        let grace = RENEWAL_GRACE.min((not_after - renew_at) / 2);
        if now < renew_at + grace || now > not_after + FORGET_AFTER {
            continue;
        }
        out.push(NotRenewing {
            name: c.meta.name.clone(),
            role,
            common_name: c.spec.common_name.clone(),
            serial: c.status.serial,
            not_after,
            expired: now >= not_after,
        });
    }
    out
}

fn warn_not_renewing(found: &[NotRenewing]) {
    for c in found {
        let what = match (c.role, c.expired) {
            (crate::ca::ROLE_AGENT, false) => {
                "agent certificate is due for renewal and has not been renewed; check that the agent on that host is running and reaches the control plane"
            }
            (crate::ca::ROLE_AGENT, true) => {
                "agent certificate has expired without being renewed; the host needs a new join token to enroll again, or revoke the certificate if the host is gone"
            }
            (_, false) => {
                "server certificate is due for renewal and has not been renewed; the server that holds it renews it while it runs"
            }
            (_, true) => {
                "server certificate has expired without being renewed; if that server is gone, delete the certificate"
            }
        };
        tracing::warn!(
            certificate = %c.name,
            name = %c.common_name,
            serial = c.serial,
            expires = %c.not_after,
            "keep: {what}"
        );
    }
}

/// Sweep now, then every [`sweep_interval`]`(grace)`, until `active` is
/// cancelled (leadership lost or shutdown).
pub async fn run(store: Store, grace: Duration, active: CancellationToken) {
    let mut tick = tokio::time::interval(sweep_interval(grace));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut warned: Option<tokio::time::Instant> = None;
    loop {
        tokio::select! {
            _ = active.cancelled() => {
                tracing::debug!("keep sweep stopping");
                return;
            }
            _ = tick.tick() => {
                let s = store.clone();
                let report = tokio::task::spawn_blocking(move || {
                    sweep_join_tokens(&s, Utc::now(), grace)
                })
                .await
                .unwrap_or(SweepReport { failed: 1, ..SweepReport::default() });
                if report.deleted > 0 || report.failed > 0 {
                    tracing::info!(
                        scanned = report.scanned,
                        deleted = report.deleted,
                        failed = report.failed,
                        "keep sweep: join tokens"
                    );
                }
                if warned.is_none_or(|at| at.elapsed() >= WARN_EVERY) {
                    let s = store.clone();
                    let found = tokio::task::spawn_blocking(move || {
                        certificates_not_renewing(&s, Utc::now())
                    })
                    .await
                    .unwrap_or_default();
                    if !found.is_empty() {
                        warn_not_renewing(&found);
                        warned = Some(tokio::time::Instant::now());
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::join::{create_join_token_with, redeem_join_token, NewJoinToken};
    use crate::quota::KeepConfig;
    use chrono::Duration as D;
    use pistack_types::Leadership;

    const WEEK: Duration = Duration::from_secs(7 * 24 * 3600);

    fn mint(store: &Store, name: &str, max_uses: Option<u64>) -> (JoinToken, String) {
        create_join_token_with(
            store,
            &NewJoinToken {
                name,
                created_by: "admin",
                multi_use: max_uses.is_some(),
                max_uses,
                ttl: None,
            },
            &KeepConfig::default(),
        )
        .unwrap()
    }

    /// Move a token's clock: expiry and last use shifted by `by`.
    fn age(store: &Store, t: &JoinToken, expires_in: D, used_ago: Option<D>) {
        store
            .collection::<JoinToken>()
            .modify(&t.meta.id, |t| {
                let now = Utc::now();
                t.spec.expires_at = now + expires_in;
                if let Some(ago) = used_ago {
                    t.status.last_used = Some(now - ago);
                }
                Ok(())
            })
            .unwrap();
    }

    fn names(store: &Store) -> Vec<String> {
        let mut v: Vec<String> = store
            .collection::<JoinToken>()
            .list(None)
            .unwrap()
            .into_iter()
            .map(|t| t.meta.name)
            .collect();
        v.sort();
        v
    }

    #[test]
    fn only_tokens_dead_for_the_whole_grace_period_go() {
        let store = Store::in_memory().unwrap();
        // Live: untouched however old.
        let (_, live_clear) = mint(&store, "live", None);
        let (partly, clear) = mint(&store, "multi-partly-used", Some(3));
        redeem_join_token(&store, &clear).unwrap();
        age(&store, &partly, D::hours(1), Some(D::days(30)));

        // Expired: kept inside the grace period, deleted after it.
        let (t, _) = mint(&store, "expired-recently", None);
        age(&store, &t, -D::days(6), None);
        let (t, _) = mint(&store, "expired-long-ago", None);
        age(&store, &t, -D::days(8), None);

        // Spent: the grace runs from the last use, not from expiry.
        let (t, clear) = mint(&store, "spent-recently", None);
        redeem_join_token(&store, &clear).unwrap();
        age(&store, &t, D::hours(1), Some(D::days(6)));
        let (t, clear) = mint(&store, "spent-long-ago", None);
        redeem_join_token(&store, &clear).unwrap();
        age(&store, &t, D::hours(1), Some(D::days(8)));
        let (t, clear) = mint(&store, "multi-used-up", Some(1));
        redeem_join_token(&store, &clear).unwrap();
        age(&store, &t, D::hours(1), Some(D::days(8)));

        // Spent long ago and expired recently: dead since the spend.
        let (t, clear) = mint(&store, "spent-then-expired", None);
        redeem_join_token(&store, &clear).unwrap();
        age(&store, &t, -D::days(1), Some(D::days(9)));

        let r = sweep_join_tokens(&store, Utc::now(), WEEK);
        assert_eq!(
            r,
            SweepReport {
                scanned: 8,
                deleted: 4,
                failed: 0
            }
        );
        assert_eq!(
            names(&store),
            [
                "expired-recently",
                "live",
                "multi-partly-used",
                "spent-recently"
            ]
        );
        // A second sweep has nothing to do.
        assert_eq!(sweep_join_tokens(&store, Utc::now(), WEEK).deleted, 0);
        // The live token still redeems.
        redeem_join_token(&store, &live_clear).unwrap();
    }

    #[test]
    fn the_boundary_is_inclusive_and_grace_zero_deletes_at_once() {
        let store = Store::in_memory().unwrap();
        let (t, _) = mint(&store, "edge", None);
        let now = Utc::now();
        store
            .collection::<JoinToken>()
            .modify(&t.meta.id, |t| {
                t.spec.expires_at = now - D::days(7);
                Ok(())
            })
            .unwrap();
        // One second short of the grace: kept. Exactly at it: deleted.
        assert_eq!(
            sweep_join_tokens(&store, now - D::seconds(1), WEEK).deleted,
            0
        );
        assert_eq!(sweep_join_tokens(&store, now, WEEK).deleted, 1);

        let (_, clear) = mint(&store, "now", None);
        redeem_join_token(&store, &clear).unwrap();
        assert_eq!(
            sweep_join_tokens(&store, Utc::now(), Duration::ZERO).deleted,
            1
        );
        assert!(names(&store).is_empty());
    }

    #[test]
    fn records_from_before_the_cap_go_only_by_expiry() {
        let store = Store::in_memory().unwrap();
        let (t, _) = mint(&store, "old", Some(5));
        store
            .collection::<JoinToken>()
            .modify(&t.meta.id, |t| {
                t.spec.max_uses = None;
                t.status.uses = 10_000;
                t.status.last_used = Some(Utc::now() - D::days(30));
                Ok(())
            })
            .unwrap();
        assert_eq!(sweep_join_tokens(&store, Utc::now(), WEEK).deleted, 0);
        age(&store, &t, -D::days(8), None);
        assert_eq!(sweep_join_tokens(&store, Utc::now(), WEEK).deleted, 1);
    }

    #[test]
    fn interval_follows_the_grace_within_bounds() {
        assert_eq!(sweep_interval(Duration::ZERO), MIN_SWEEP_INTERVAL);
        assert_eq!(
            sweep_interval(Duration::from_secs(600)),
            Duration::from_secs(600)
        );
        assert_eq!(sweep_interval(WEEK), MAX_SWEEP_INTERVAL);
    }

    async fn wait_until(mut f: impl FnMut() -> bool) -> bool {
        for _ in 0..100 {
            if f() {
                return true;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        false
    }

    #[tokio::test]
    async fn runs_only_while_leader() {
        let store = Store::in_memory().unwrap();
        let (t, _) = mint(&store, "dead", None);
        age(&store, &t, -D::days(30), None);
        let leadership = Leadership::new(false);
        let shutdown = CancellationToken::new();
        let task = {
            let (store, leadership, shutdown) =
                (store.clone(), leadership.clone(), shutdown.clone());
            tokio::spawn(async move {
                leadership
                    .run_while_leader(shutdown, move |active| run(store.clone(), WEEK, active))
                    .await
            })
        };
        // A follower never sweeps.
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        assert_eq!(names(&store), ["dead"]);
        // Promotion sweeps at once.
        leadership.set(true);
        assert!(wait_until(|| names(&store).is_empty()).await);
        // Demoted: a newly dead token stays, even across what would be a sweep.
        leadership.set(false);
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        let (t, _) = mint(&store, "dead-2", None);
        age(&store, &t, -D::days(30), None);
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        assert_eq!(names(&store), ["dead-2"]);
        leadership.set(true);
        assert!(wait_until(|| names(&store).is_empty()).await);
        shutdown.cancel();
        task.await.unwrap();
    }

    #[test]
    fn a_certificate_past_its_renewal_point_with_no_successor_is_reported() {
        let store = Store::in_memory().unwrap();
        let c = crate::Client::open(&store).unwrap();
        let serial = |pki: &crate::Pki| {
            crate::ca::serial_from_der(&crate::ca::pem_to_der(&pki.cert_pem).unwrap()).unwrap()
        };
        let quiet = serial(&c.issue_client_cert("quiet").unwrap());
        let busy = serial(&c.issue_client_cert("busy").unwrap());
        let gone = serial(&c.issue_client_cert("gone").unwrap());
        c.issue_server_cert("pistack", &["10.0.0.11".to_owned()])
            .unwrap();
        // One a project asked for: never the cloud's to watch.
        let project = pistack_types::Id::new(pistack_types::kind::PROJECT);
        c.issue_certificate(
            Some(project),
            "web",
            crate::CertificateSpec {
                common_name: "web".into(),
                ..Default::default()
            },
        )
        .unwrap();

        let rec = c.managed_certificate(quiet).unwrap().unwrap();
        let (nb, na) = (
            rec.status.not_before.unwrap(),
            rec.status.not_after.unwrap(),
        );
        let due = crate::ca::renew_at(nb, na);
        let names = |now| {
            let mut v: Vec<String> = certificates_not_renewing(&store, now)
                .into_iter()
                .map(|n| format!("{}:{}", n.role, n.common_name))
                .collect();
            v.sort();
            v
        };

        // Fresh, at the renewal point, and inside the grace: nothing yet.
        assert!(names(Utc::now()).is_empty());
        assert!(names(due).is_empty());
        assert!(names(due + RENEWAL_GRACE - D::seconds(1)).is_empty());
        // Past it: everything that has not been renewed, the server's too.
        let late = due + RENEWAL_GRACE + D::seconds(1);
        assert_eq!(
            names(late),
            ["agent:busy", "agent:gone", "agent:quiet", "server:pistack"]
        );

        // A renewed one is no longer reported and neither is a revoked one.
        let req = crate::ca::new_key_request("busy").unwrap();
        c.renew_certificate(busy, "busy", &req.csr_pem).unwrap();
        let id = c.managed_certificate(gone).unwrap().unwrap().meta.id;
        c.revoke_certificate(&id).unwrap();
        let serials: Vec<u64> = certificates_not_renewing(&store, late)
            .iter()
            .map(|n| n.serial)
            .collect();
        assert!(serials.contains(&quiet));
        assert!(!serials.contains(&busy));
        assert!(!serials.contains(&gone));

        // Expired is said, and after a month it is dropped.
        let found = certificates_not_renewing(&store, na + D::seconds(1));
        assert!(found.iter().all(|n| n.expired));
        assert!(found.iter().any(|n| n.serial == quiet));
        assert!(!certificates_not_renewing(&store, na - D::seconds(1))
            .iter()
            .any(|n| n.expired));
        assert!(names(na + FORGET_AFTER + D::days(1)).is_empty());
    }
}
