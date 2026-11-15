// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Shared filesystems. A share is a name, a protocol (`nfs` or `cephfs`),
//! a size and a list of who may mount it.
//!
//! `pi share access` edits that list. The shelf API has no endpoint for it:
//! a rule is added or dropped by sending the whole `spec` back with
//! `If-Match` set to the share's current revision, so an access edit that
//! races a second one fails rather than overwriting it.

use super::args::{DeleteArgs, ListArgs, MetaArgs, NameArg};
use super::client::{CliError, CliResult};
use super::commands::{create_body, do_create, do_delete, do_list, do_show, kind};
use super::kinds::KindDef;
use super::output::{print_one, str_at, table, Output};
use super::{Ctx, Io};
use clap::{Args, Subcommand};
use serde_json::{json, Value};

#[derive(Subcommand, Debug)]
pub enum ShareCmd {
    /// List.
    #[command(alias = "ls")]
    List(ListArgs),
    /// Show one by name or id.
    #[command(alias = "get")]
    Show(NameArg),
    /// Create.
    Create(ShareCreate),
    /// Delete by name or id.
    #[command(alias = "rm")]
    Delete(DeleteArgs),
    /// Who may mount this share.
    Access {
        #[command(subcommand)]
        cmd: ShareAccessCmd,
    },
}

#[derive(Args, Debug)]
pub struct ShareCreate {
    pub name: String,
    #[command(flatten)]
    pub meta: MetaArgs,
    /// Protocol: nfs or cephfs.
    #[arg(long, default_value = "nfs")]
    pub protocol: String,
    /// Size in GiB.
    #[arg(long = "size", value_name = "GIB", default_value_t = 10)]
    pub size_gib: u32,
    /// Allow a client CIDR to mount (repeatable).
    #[arg(long = "cidr", value_name = "CIDR")]
    pub cidrs: Vec<String>,
    /// Mode for the CIDRs given: read or write.
    #[arg(long, default_value = "write")]
    pub mode: String,
}

#[derive(Subcommand, Debug)]
pub enum ShareAccessCmd {
    /// The rules on a share.
    #[command(alias = "ls")]
    List(NameArg),
    /// Allow a client CIDR to mount.
    Allow(AccessAllow),
    /// Remove a client CIDR.
    #[command(alias = "rm")]
    Deny(AccessDeny),
}

#[derive(Args, Debug)]
pub struct AccessAllow {
    /// Share (name or id).
    pub name: String,
    /// Client CIDR, for example 10.0.0.0/24.
    #[arg(long)]
    pub cidr: String,
    /// Mount mode for the rule.
    #[arg(long, default_value = "write")]
    pub mode: String,
}

#[derive(Args, Debug)]
pub struct AccessDeny {
    /// Share (name or id).
    pub name: String,
    /// Client CIDR, for example 10.0.0.0/24.
    #[arg(long)]
    pub cidr: String,
}

const ACCESS_HEADERS: [&str; 2] = ["CIDR", "MODE"];

pub async fn share(ctx: &Ctx, cmd: ShareCmd, io: &mut Io<'_>) -> CliResult<()> {
    let k = kind("share");
    match cmd {
        ShareCmd::List(a) => do_list(ctx, io, k, a).await,
        ShareCmd::Show(a) => do_show(ctx, io, k, a).await,
        ShareCmd::Delete(a) => do_delete(ctx, io, k, a).await,
        ShareCmd::Create(a) => {
            let mut access = Vec::with_capacity(a.cidrs.len());
            for cidr in &a.cidrs {
                access.push(rule(cidr, &a.mode)?);
            }
            let spec = json!({
                "name": a.name,
                "protocol": a.protocol,
                "size_gib": a.size_gib,
                "access": access,
            });
            let body = create_body(k, &a.name, &a.meta, spec)?;
            do_create(ctx, io, k, body, false).await.map(|_| ())
        }
        ShareCmd::Access { cmd } => access(ctx, io, k, cmd).await,
    }
}

async fn access(
    ctx: &Ctx,
    io: &mut Io<'_>,
    k: &'static KindDef,
    cmd: ShareAccessCmd,
) -> CliResult<()> {
    match cmd {
        ShareAccessCmd::List(a) => {
            let c = ctx.client()?;
            let share = c.get_value(&k.item_path(&a.name)).await?;
            print_access(io, ctx, &share)?;
            Ok(())
        }
        ShareAccessCmd::Allow(a) => {
            write_access(ctx, io, k, &a.name, &a.cidr, Some(&a.mode), true).await
        }
        ShareAccessCmd::Deny(a) => write_access(ctx, io, k, &a.name, &a.cidr, None, false).await,
    }
}

fn print_access(io: &mut Io<'_>, ctx: &Ctx, share: &Value) -> CliResult<()> {
    if !is_tabular(ctx.fmt.output) {
        return Ok(print_one(io.stdout, ctx.fmt, &access_of(share))?);
    }
    let rows: Vec<Vec<String>> = access_of(share)
        .as_array()
        .map(|rules| {
            rules
                .iter()
                .map(|r| {
                    vec![
                        str_at(r, "/cidr"),
                        r.get("mode")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_owned(),
                    ]
                })
                .collect()
        })
        .unwrap_or_default();
    write!(
        io.stdout,
        "{}",
        table(&ACCESS_HEADERS, &rows, ctx.fmt.no_headers)
    )?;
    Ok(())
}

/// The CIDR rules of a share, as `[{cidr, mode}]`.
fn access_of(share: &Value) -> Value {
    let mut out = Vec::new();
    for rule in share
        .pointer("/spec/access")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if rule.get("cidr").and_then(Value::as_str).is_some() {
            let mut kept = serde_json::Map::new();
            kept.insert("cidr".into(), rule["cidr"].clone());
            if let Some(mode) = rule.get("mode") {
                kept.insert("mode".into(), mode.clone());
            }
            out.push(Value::Object(kept));
        }
    }
    Value::Array(out)
}

fn is_tabular(fmt: Output) -> bool {
    matches!(fmt, Output::Table | Output::Wide)
}

/// Add, change or drop one CIDR rule. `mode` is given only when allowing;
/// denying names the CIDR alone.
async fn write_access(
    ctx: &Ctx,
    io: &mut Io<'_>,
    k: &'static KindDef,
    name: &str,
    cidr: &str,
    mode: Option<&str>,
    allow: bool,
) -> CliResult<()> {
    let c = ctx.client()?;
    let cur = c.get_value(&k.item_path(name)).await?;
    let id = str_at(&cur, "/id");
    let mut spec = cur.get("spec").cloned().unwrap_or_else(|| json!({}));
    if spec
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("")
        .is_empty()
    {
        spec["name"] = json!(str_at(&cur, "/name"));
    }
    let mut access: Vec<Value> = spec
        .get("access")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let pos = access
        .iter()
        .position(|r| r.get("cidr").and_then(Value::as_str) == Some(cidr));
    if allow {
        let rule = rule(cidr, mode.unwrap_or("write"))?;
        match pos {
            Some(i) => access[i] = rule,
            None => access.push(rule),
        }
    } else {
        let Some(i) = pos else {
            return Err(CliError::usage(format!(
                "share '{name}' has no rule for '{cidr}'"
            )));
        };
        access.remove(i);
    }
    spec["access"] = Value::Array(access);
    let body = json!({ "spec": spec });
    let path = k.item_path(&id);
    let updated = match str_at(&cur, "/revision").as_str() {
        "" => c.put(&path, &body).await?,
        rev => c.put_if_match(&path, &body, rev).await?,
    };
    if !is_tabular(ctx.fmt.output) {
        return Ok(print_one(io.stdout, ctx.fmt, &access_of(&updated))?);
    }
    let (what, preposition) = if allow {
        ("allowed", "for")
    } else {
        ("denied", "to")
    };
    writeln!(
        io.stdout,
        "access {} \"{}\" {what} {} {}",
        k.noun,
        str_at(&updated, "/name"),
        preposition,
        cidr
    )?;
    Ok(())
}

/// One access rule. `mode` is `read` or `write`; a share's rules are keyed by
/// CIDR, so one rule per CIDR.
fn rule(cidr: &str, mode: &str) -> CliResult<Value> {
    if !cidr_ok(cidr) {
        return Err(CliError::usage(format!(
            "'{cidr}' is not a CIDR, e.g. 10.0.0.0/24"
        )));
    }
    let mode = match mode.trim().to_ascii_lowercase().as_str() {
        "read" => "read",
        "write" => "write",
        other => {
            return Err(CliError::usage(format!(
                "mode must be read or write, got '{other}'"
            )))
        }
    };
    Ok(json!({ "cidr": cidr.trim(), "mode": mode }))
}

/// `10.0.0.0/24` or `fd00::/64`. The server checks this too; catching it
/// here means the share is not left pending on a typo.
fn cidr_ok(cidr: &str) -> bool {
    let cidr = cidr.trim();
    let Some((addr, bits)) = cidr.split_once('/') else {
        return false;
    };
    let Ok(ip) = addr.parse::<std::net::IpAddr>() else {
        return false;
    };
    match bits.parse::<u8>() {
        Ok(n) if ip.is_ipv6() => n <= 128,
        Ok(n) => n <= 32,
        Err(_) => false,
    }
}
