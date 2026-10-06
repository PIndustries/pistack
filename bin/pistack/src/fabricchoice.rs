// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Keep every server of a cluster on the same fabric backend.
//!
//! `PISTACK_FABRIC_BACKEND` is read by several modules of one process. A
//! server started without it used to run the mock fabric, so a cluster where
//! one server forgot the setting would stop wiring real networks the moment
//! that server became leader. Now the choice is saved in the cluster store:
//! a leader with an explicit setting saves it, and a server without one
//! adopts the saved choice before any module reads the variable.
//!
//! Only the leader writes: at startup if it leads then, otherwise when it
//! is promoted. A follower's store is a
//! replica: a local write there is an entry the leader never had, and the
//! follow loop rightly stops replicating into a store that has split off.
//! A follower uses its own explicit setting and warns if it differs.

use pistack_store::Store;

pub const FABRIC_BACKEND_ENV: &str = "PISTACK_FABRIC_BACKEND";

/// Meta key holding the cluster's fabric backend.
const META_KEY: &str = "fabric_backend";

/// Values a server may set. `mock` is an explicit opt-out that is also
/// saved, so a laptop cluster can say so on purpose.
const KNOWN: [&str; 4] = ["hostnat", "overlay", "ovn", "mock"];

#[derive(Debug, PartialEq, Eq)]
pub enum Choice {
    /// Use the explicit setting and save it.
    Explicit {
        backend: String,
        replaces: Option<String>,
    },
    /// No usable setting here; use the saved one.
    Adopt(String),
    /// Nothing set and nothing saved: the mock fabric, as before.
    Unset,
}

/// Decide from this server's setting and the cluster's saved choice.
/// Pure, so it is tested without touching the process environment.
pub fn decide(env: Option<&str>, saved: Option<&str>) -> Choice {
    let env = env.map(str::trim).filter(|v| KNOWN.contains(v));
    let saved = saved.map(str::trim).filter(|v| KNOWN.contains(v));
    match (env, saved) {
        (Some(e), s) => Choice::Explicit {
            backend: e.to_owned(),
            replaces: s.filter(|s| *s != e).map(str::to_owned),
        },
        (None, Some(s)) => Choice::Adopt(s.to_owned()),
        (None, None) => Choice::Unset,
    }
}

/// Settle this process's fabric backend against the cluster store. Call
/// once membership is established and before modules are assembled.
/// `leader` is whether this server leads right now; only then is an
/// explicit choice saved.
pub fn settle(store: &Store, leader: bool) -> anyhow::Result<Choice> {
    let raw = std::env::var(FABRIC_BACKEND_ENV).ok();
    let choice = record(store, raw.as_deref(), leader)?;
    match &choice {
        Choice::Explicit { backend, .. } | Choice::Adopt(backend) => {
            std::env::set_var(FABRIC_BACKEND_ENV, backend)
        }
        Choice::Unset => std::env::remove_var(FABRIC_BACKEND_ENV),
    }
    Ok(choice)
}

/// Save this server's explicit choice each time it becomes leader. A
/// promoted leader logs its writes before the leadership signal goes up,
/// so the save reaches every follower. Runs until `shutdown`.
pub fn save_on_promotion(
    store: Store,
    leadership: pistack_types::Leadership,
    shutdown: pistack_types::CancellationToken,
    choice: &Choice,
) {
    let Choice::Explicit { backend, .. } = choice else {
        return;
    };
    let backend = backend.clone();
    let watch = leadership.clone();
    tokio::spawn(async move {
        watch
            .run_while_leader(shutdown, move |_active| {
                let store = store.clone();
                let backend = backend.clone();
                let leadership = leadership.clone();
                async move {
                    if !leadership.is_leader() {
                        return;
                    }
                    if let Err(e) = save_as_leader(&store, &backend) {
                        tracing::warn!(error = %e, "fabric: saving backend on promotion");
                    }
                }
            })
            .await;
    });
}

/// Save `backend` unless it is already the saved choice. Only call on the
/// leader. Returns whether it wrote.
fn save_as_leader(store: &Store, backend: &str) -> anyhow::Result<bool> {
    let saved: Option<String> = store.get_meta(META_KEY)?;
    if saved.as_deref() == Some(backend) {
        return Ok(false);
    }
    tracing::info!(
        backend,
        replaces = ?saved,
        "fabric: became leader; saving this server's backend"
    );
    store.put_meta(META_KEY, &backend)?;
    Ok(true)
}

/// Decide against the store, save an explicit choice when `leader`, and
/// log. Never writes on a follower. Leaves the process environment alone
/// so tests can run it in parallel.
fn record(store: &Store, raw: Option<&str>, leader: bool) -> anyhow::Result<Choice> {
    if let Some(v) = raw.map(str::trim).filter(|v| !v.is_empty()) {
        if !KNOWN.contains(&v) {
            tracing::error!(
                value = v,
                "{FABRIC_BACKEND_ENV} is not hostnat, overlay, ovn or mock; ignoring it"
            );
        }
    }
    let saved: Option<String> = store.get_meta(META_KEY)?;
    let choice = decide(raw, saved.as_deref());
    match &choice {
        Choice::Explicit { backend, replaces } => {
            if let Some(old) = replaces {
                let action = if leader {
                    "using and saving this server's setting"
                } else {
                    "using this server's setting; only the leader saves it"
                };
                tracing::warn!(
                    saved = %old,
                    set = %backend,
                    "fabric backend differs from the cluster's saved choice; \
                     {action}. Set the same {FABRIC_BACKEND_ENV} on every server"
                );
            }
            if leader && saved.as_deref() != Some(backend.as_str()) {
                store.put_meta(META_KEY, backend)?;
            }
            tracing::info!(backend = %backend, "fabric: backend from this server's setting");
        }
        Choice::Adopt(backend) => tracing::warn!(
            backend = %backend,
            "fabric: {FABRIC_BACKEND_ENV} not set here; using the cluster's saved choice"
        ),
        Choice::Unset => {
            tracing::info!("fabric: no backend set or saved; using the mock fabric")
        }
    }
    Ok(choice)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_explicit_setting_wins_and_says_what_it_replaces() {
        assert_eq!(
            decide(Some("overlay"), None),
            Choice::Explicit {
                backend: "overlay".into(),
                replaces: None
            }
        );
        assert_eq!(
            decide(Some(" overlay "), Some("overlay")),
            Choice::Explicit {
                backend: "overlay".into(),
                replaces: None
            }
        );
        assert_eq!(
            decide(Some("overlay"), Some("hostnat")),
            Choice::Explicit {
                backend: "overlay".into(),
                replaces: Some("hostnat".into())
            }
        );
        assert_eq!(
            decide(Some("mock"), Some("overlay")),
            Choice::Explicit {
                backend: "mock".into(),
                replaces: Some("overlay".into())
            }
        );
    }

    #[test]
    fn a_server_without_a_setting_adopts_the_saved_choice() {
        assert_eq!(
            decide(None, Some("overlay")),
            Choice::Adopt("overlay".into())
        );
        assert_eq!(
            decide(Some(""), Some("hostnat")),
            Choice::Adopt("hostnat".into())
        );
        assert_eq!(
            decide(Some("vxlan"), Some("overlay")),
            Choice::Adopt("overlay".into())
        );
        assert_eq!(decide(None, None), Choice::Unset);
        assert_eq!(decide(Some("bogus"), Some("bogus")), Choice::Unset);
    }

    #[test]
    fn record_saves_an_explicit_choice_and_adopts_it_later() {
        let store = Store::in_memory().unwrap();
        assert_eq!(record(&store, None, true).unwrap(), Choice::Unset);
        assert_eq!(store.get_meta::<String>(META_KEY).unwrap(), None);

        record(&store, Some("overlay"), true).unwrap();
        assert_eq!(
            store.get_meta::<String>(META_KEY).unwrap().as_deref(),
            Some("overlay")
        );
        assert_eq!(
            record(&store, None, false).unwrap(),
            Choice::Adopt("overlay".into())
        );
        assert_eq!(
            record(&store, Some("typo"), false).unwrap(),
            Choice::Adopt("overlay".into())
        );

        record(&store, Some("hostnat"), true).unwrap();
        assert_eq!(
            store.get_meta::<String>(META_KEY).unwrap().as_deref(),
            Some("hostnat")
        );
    }

    #[test]
    fn a_promoted_leader_saves_its_setting_once() {
        let store = Store::in_memory().unwrap();
        record(&store, Some("hostnat"), true).unwrap();
        assert!(save_as_leader(&store, "overlay").unwrap());
        assert_eq!(
            store.get_meta::<String>(META_KEY).unwrap().as_deref(),
            Some("overlay")
        );
        assert!(!save_as_leader(&store, "overlay").unwrap());
    }

    #[test]
    fn a_follower_never_writes_its_replica() {
        let store = Store::in_memory().unwrap();
        // Nothing saved yet: the follower uses its setting but saves nothing.
        assert_eq!(
            record(&store, Some("overlay"), false).unwrap(),
            Choice::Explicit {
                backend: "overlay".into(),
                replaces: None
            }
        );
        assert_eq!(store.get_meta::<String>(META_KEY).unwrap(), None);
        // A different saved choice: still no write, the saved one stays.
        record(&store, Some("hostnat"), true).unwrap();
        record(&store, Some("overlay"), false).unwrap();
        assert_eq!(
            store.get_meta::<String>(META_KEY).unwrap().as_deref(),
            Some("hostnat")
        );
    }
}
