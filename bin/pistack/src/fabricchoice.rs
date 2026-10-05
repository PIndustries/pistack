// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Keep every server of a cluster on the same fabric backend.
//!
//! `PISTACK_FABRIC_BACKEND` is read by several modules of one process. A
//! server started without it used to run the mock fabric, so a cluster where
//! one server forgot the setting would stop wiring real networks the moment
//! that server became leader. Now the choice is saved in the cluster store:
//! a server with an explicit setting saves it, and a server without one
//! adopts the saved choice before any module reads the variable.

use pistack_store::Store;

pub const FABRIC_BACKEND_ENV: &str = "PISTACK_FABRIC_BACKEND";

/// Meta key holding the cluster's fabric backend.
const META_KEY: &str = "fabric_backend";

/// Values a server may set. `mock` is an explicit opt-out that is also
/// saved, so a laptop cluster can say so on purpose.
const KNOWN: [&str; 3] = ["hostnat", "overlay", "mock"];

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
pub fn settle(store: &Store) -> anyhow::Result<Choice> {
    let raw = std::env::var(FABRIC_BACKEND_ENV).ok();
    let choice = record(store, raw.as_deref())?;
    match &choice {
        Choice::Explicit { backend, .. } | Choice::Adopt(backend) => {
            std::env::set_var(FABRIC_BACKEND_ENV, backend)
        }
        Choice::Unset => std::env::remove_var(FABRIC_BACKEND_ENV),
    }
    Ok(choice)
}

/// Decide against the store, save an explicit choice and log. Leaves the
/// process environment alone so tests can run it in parallel.
fn record(store: &Store, raw: Option<&str>) -> anyhow::Result<Choice> {
    if let Some(v) = raw.map(str::trim).filter(|v| !v.is_empty()) {
        if !KNOWN.contains(&v) {
            tracing::error!(
                value = v,
                "{FABRIC_BACKEND_ENV} is not hostnat, overlay or mock; ignoring it"
            );
        }
    }
    let saved: Option<String> = store.get_meta(META_KEY)?;
    let choice = decide(raw, saved.as_deref());
    match &choice {
        Choice::Explicit { backend, replaces } => {
            if let Some(old) = replaces {
                tracing::warn!(
                    saved = %old,
                    set = %backend,
                    "fabric backend differs from the cluster's saved choice; \
                     using and saving this server's setting. Set the same \
                     {FABRIC_BACKEND_ENV} on every server"
                );
            }
            if saved.as_deref() != Some(backend.as_str()) {
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
        assert_eq!(record(&store, None).unwrap(), Choice::Unset);
        assert_eq!(store.get_meta::<String>(META_KEY).unwrap(), None);

        record(&store, Some("overlay")).unwrap();
        assert_eq!(
            store.get_meta::<String>(META_KEY).unwrap().as_deref(),
            Some("overlay")
        );
        assert_eq!(
            record(&store, None).unwrap(),
            Choice::Adopt("overlay".into())
        );
        assert_eq!(
            record(&store, Some("typo")).unwrap(),
            Choice::Adopt("overlay".into())
        );

        record(&store, Some("hostnat")).unwrap();
        assert_eq!(
            store.get_meta::<String>(META_KEY).unwrap().as_deref(),
            Some("hostnat")
        );
    }
}
