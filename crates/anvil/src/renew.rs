// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The agent's client certificate: where it is kept and how it is renewed.
//!
//! The certificate, its key and the CA bundle are three files under
//! `<data_dir>/agent`. Once two thirds of the certificate's lifetime have
//! passed ([`keep::ca::renew_at`]) the agent makes a new key, asks the
//! control plane to certify it over the connection the current certificate
//! secures, and checks that the server accepts the answer before it uses
//! it. Until that check passes the old certificate stays on disk and in the
//! running client, so a renewal that fails half way costs nothing and is
//! tried again. The new private key is made here and is never sent.
//!
//! The new set is staged in `agent/next` and moved into place only after
//! the server accepted it ([`keep::files`]), so a crash in the middle
//! cannot leave a certificate beside the wrong key.
//!
//! A certificate that has already expired cannot renew itself: the server
//! no longer takes it. The agent then enrolls again with its join token if
//! it was started with one, and otherwise says so in the log until it is.
//!
//! All of this is for an agent that talks HTTPS. On plain HTTP a
//! certificate is never shown, so there is nothing to renew it with and
//! nothing that would notice it expiring.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Context;
use chrono::{DateTime, Utc};
use keep::files;
use keep::Pki;
use pistack_rpc::AgentClient;
use tokio_util::sync::CancellationToken;

/// Pause before the first look after start, between looks, and after a
/// renewal that did not go through.
const FIRST_CHECK: Duration = Duration::from_secs(30);
const CHECK_EVERY: Duration = Duration::from_secs(60 * 60);
const RETRY_AFTER: Duration = Duration::from_secs(5 * 60);

/// Validity of the leaf certificate in `cert_pem`.
pub fn validity(cert_pem: &str) -> anyhow::Result<(DateTime<Utc>, DateTime<Utc>)> {
    Ok(keep::ca::validity(&keep::ca::pem_to_der(cert_pem)?)?)
}

/// What one look at the certificate did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Renewal {
    /// This agent has no certificate (plain HTTP in a lab).
    NoCertificate,
    /// It has one but talks plain HTTP, where a certificate is never
    /// shown. There is nothing to prove a renewal with, and nothing uses
    /// the certificate either.
    NotPresented,
    /// Still in the first two thirds of its lifetime.
    NotDue {
        renew_at: DateTime<Utc>,
        not_after: DateTime<Utc>,
    },
    /// A new certificate is on disk and in use.
    Renewed {
        serial: u64,
        not_after: DateTime<Utc>,
    },
    /// Too late to renew: enroll again.
    Expired { not_after: DateTime<Utc> },
}

/// Renew the certificate in `dir` if it is due at `now`, and switch
/// `client` (and every clone of it) to the new one.
pub async fn renew_if_due(
    client: &AgentClient,
    dir: &Path,
    now: DateTime<Utc>,
) -> anyhow::Result<Renewal> {
    let Some(pki) = files::load(dir)? else {
        return Ok(Renewal::NoCertificate);
    };
    if !client.is_secure() {
        return Ok(Renewal::NotPresented);
    }
    let (not_before, not_after) = validity(&pki.cert_pem)?;
    if now >= not_after {
        return Ok(Renewal::Expired { not_after });
    }
    let renew_at = keep::ca::renew_at(not_before, not_after);
    if now < renew_at {
        return Ok(Renewal::NotDue {
            renew_at,
            not_after,
        });
    }
    let request = keep::ca::new_key_request(client.node_name())?;
    let answer = client
        .renew(&request.csr_pem)
        .await
        .context("asking for a new certificate")?;
    let new = Pki {
        cert_pem: answer.cert_pem,
        key_pem: request.key_pem,
        ca_pem: answer.ca_pem,
    };
    adopt(client, dir, &new).await
}

/// Enroll again with a join token, for an agent whose certificate has
/// expired while it was running.
pub async fn enroll_again(
    client: &AgentClient,
    dir: &Path,
    token: &str,
) -> anyhow::Result<Renewal> {
    let node = client.node_name();
    // No certificate to show, but the CA on disk still says which server
    // to believe.
    let bootstrap = match files::load(dir)? {
        Some(pki) => AgentClient::with_ca(client.base_url(), node, &pki.ca_pem)?,
        None => AgentClient::new(client.base_url(), node)?,
    }
    .with_token(token);
    bootstrap.learn_servers(client.known_servers());
    let request = keep::ca::new_key_request(node)?;
    let enrolled = bootstrap
        .enroll(node, Some(&request.csr_pem))
        .await
        .context("enrolling with the join token")?;
    let new = enrolled_pki(enrolled, request.key_pem);
    adopt(client, dir, &new).await
}

/// The certificate an enroll answer carries, with the key that belongs to
/// it: ours, unless the server is from before certificate requests and
/// made one itself.
pub(crate) fn enrolled_pki(enrolled: pistack_rpc::EnrollResponse, our_key: String) -> Pki {
    Pki {
        cert_pem: enrolled.cert_pem,
        key_pem: if enrolled.key_pem.trim().is_empty() {
            our_key
        } else {
            enrolled.key_pem
        },
        ca_pem: enrolled.ca_pem,
    }
}

/// Make `new` the agent's certificate once the server has accepted it.
/// Until then the old one stays on disk and in `client`.
async fn adopt(client: &AgentClient, dir: &Path, new: &Pki) -> anyhow::Result<Renewal> {
    let der = keep::ca::pem_to_der(&new.cert_pem)?;
    let serial = keep::ca::serial_from_der(&der)?;
    let (_, not_after) = keep::ca::validity(&der)?;
    if keep::ca::common_name_from_der(&der)? != client.node_name() {
        anyhow::bail!("the new certificate is not for node {}", client.node_name());
    }
    files::stage(dir, new)?;

    let probe = client.with_identity(&new.ca_pem, new)?;
    let seen = probe
        .whoami()
        .await
        .context("the server did not accept the new certificate; keeping the old one")?;
    if seen.node_name != client.node_name() || seen.cert_serial != Some(serial) {
        anyhow::bail!(
            "the server saw node {} serial {:?} on the new certificate, expected {} serial {serial}; keeping the old one",
            seen.node_name,
            seen.cert_serial,
            client.node_name()
        );
    }
    files::commit(dir)?;
    client.use_identity(&new.ca_pem, new)?;
    Ok(Renewal::Renewed { serial, not_after })
}

/// Look at the certificate now and then, renewing it when it is due,
/// until `shutdown`. `token` is the join token the agent was started with,
/// used only when the certificate has expired.
pub async fn run(
    client: AgentClient,
    dir: PathBuf,
    token: Option<String>,
    shutdown: CancellationToken,
) {
    let mut wait = FIRST_CHECK;
    let mut told = false;
    loop {
        tokio::select! {
            _ = shutdown.cancelled() => return,
            _ = tokio::time::sleep(wait) => {}
        }
        wait = match renew_if_due(&client, &dir, Utc::now()).await {
            Ok(Renewal::NoCertificate) => CHECK_EVERY,
            Ok(Renewal::NotPresented) => {
                if !told {
                    tracing::info!(
                        "anvil agent: on plain HTTP the client certificate is not shown, so it is not renewed"
                    );
                    told = true;
                }
                CHECK_EVERY
            }
            Ok(Renewal::NotDue {
                renew_at,
                not_after,
            }) => {
                if !told {
                    tracing::info!(
                        expires = %not_after,
                        renews_after = %renew_at,
                        "anvil agent: client certificate"
                    );
                    told = true;
                }
                CHECK_EVERY
            }
            Ok(Renewal::Renewed { serial, not_after }) => {
                tracing::info!(serial, expires = %not_after, "anvil agent: client certificate renewed");
                told = false;
                CHECK_EVERY
            }
            Ok(Renewal::Expired { not_after }) => {
                expired(&client, &dir, token.as_deref(), not_after).await;
                RETRY_AFTER
            }
            Err(e) => {
                tracing::warn!(
                    error = %format!("{e:#}"),
                    retry_in = ?RETRY_AFTER,
                    "anvil agent: certificate renewal failed; the current certificate stays in use"
                );
                RETRY_AFTER
            }
        };
    }
}

async fn expired(client: &AgentClient, dir: &Path, token: Option<&str>, not_after: DateTime<Utc>) {
    let Some(token) = token else {
        tracing::error!(
            expired = %not_after,
            "anvil agent: client certificate has expired and cannot renew itself; \
             create a join token (pi join-token create) and restart the agent with --token to enroll again"
        );
        return;
    };
    tracing::error!(
        expired = %not_after,
        "anvil agent: client certificate has expired and cannot renew itself; enrolling again with the join token"
    );
    match enroll_again(client, dir, token).await {
        Ok(Renewal::Renewed { serial, not_after }) => {
            tracing::info!(serial, expires = %not_after, "anvil agent: enrolled again");
        }
        Ok(_) => {}
        Err(e) => tracing::error!(
            error = %format!("{e:#}"),
            "anvil agent: enrolling again failed; the join token may be spent or expired, start the agent with a new one"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_old_server_that_made_the_key_is_believed() {
        let grant = |key: &str| pistack_rpc::EnrollResponse {
            cert_pem: "c".into(),
            key_pem: key.into(),
            ca_pem: "ca".into(),
            grant: keep::JoinGrant {
                token: pistack_types::Id::new("jtok"),
                token_name: "t".into(),
                created_by: "admin".into(),
                expires_at: Utc::now(),
                remaining_uses: None,
            },
        };
        assert_eq!(enrolled_pki(grant(""), "ours".into()).key_pem, "ours");
        assert_eq!(
            enrolled_pki(grant("theirs"), "ours".into()).key_pem,
            "theirs"
        );
    }
}
