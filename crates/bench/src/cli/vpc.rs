// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! VPCs and the subnets cut from them: `pi vpc` and `pi vpc subnet`, for the
//! routed network spaces the fabric module serves.
//!
//! Everything the noun needs lives here: argument types and handlers. The
//! VPC itself is an ordinary kind in the table (`/network/vpcs`), so `list`,
//! `show` and `delete` are the generic verbs. Its subnets are reached through
//! the VPC: `GET`/`POST /network/vpcs/{vpc}/subnets` and
//! `DELETE /network/vpcs/{vpc}/subnets/{subnet}`. A subnet listed under a VPC
//! prints with the subnet kind's own columns; one created there is a network
//! and its one subnet made together, from a block or a bare prefix length.

use super::args::{DeleteArgs, ListArgs, MetaArgs, NameArg};
use super::client::{CliError, CliResult, Client};
use super::commands;
use super::kinds::KindDef;
use super::output::print_list;
use super::{Ctx, Io};
use clap::{Args, Subcommand};
use serde_json::{json, Map, Value};

/// A VPC's own id prefix, so a name is only resolved by `GET` when it is not
/// already an id. Kept in step with the `vpc` row in the kind table.
fn vpc_kind() -> &'static KindDef {
    commands::kind("vpc")
}

fn subnet_kind() -> &'static KindDef {
    commands::kind("subnet")
}

// ---- argument types -------------------------------------------------------------

#[derive(Subcommand, Debug)]
pub enum VpcCmd {
    /// List VPCs.
    #[command(alias = "ls")]
    List(ListArgs),
    /// Show one VPC by name or id.
    #[command(alias = "get")]
    Show(NameArg),
    /// Create a VPC.
    Create(VpcCreate),
    /// Delete a VPC and everything under it.
    #[command(alias = "rm")]
    Delete(DeleteArgs),
    /// Subnets cut from a VPC's blocks.
    Subnet {
        #[command(subcommand)]
        cmd: SubnetCmd,
    },
}

#[derive(Subcommand, Debug)]
pub enum SubnetCmd {
    /// List a VPC's subnets.
    #[command(alias = "ls")]
    List(VpcArg),
    /// Create a subnet in a VPC.
    Create(SubnetCreate),
    /// Delete a subnet from a VPC.
    #[command(alias = "rm")]
    Delete(SubnetDelete),
}

#[derive(Args, Debug)]
pub struct VpcCreate {
    /// Name the VPC is filed under.
    pub name: String,
    #[command(flatten)]
    pub meta: MetaArgs,
    /// Address block the space is made of (repeatable, at least one), e.g.
    /// 10.0.0.0/16. Subnets are cut from these.
    #[arg(long = "cidr", value_name = "CIDR", required = true)]
    pub cidrs: Vec<String>,
    /// External network: where traffic leaves and floating IPs are drawn from
    /// (name or id).
    #[arg(long, value_name = "NET")]
    pub external_network: Option<String>,
    /// DNS domain handed to a subnet on this VPC's networks that sets none.
    #[arg(long, value_name = "DOMAIN")]
    pub dns_domain: Option<String>,
}

#[derive(Args, Debug)]
pub struct VpcArg {
    /// The VPC (name or id).
    pub vpc: String,
}

#[derive(Args, Debug)]
pub struct SubnetCreate {
    /// The VPC (name or id).
    pub vpc: String,
    /// Name the subnet is filed under.
    pub name: String,
    /// A full CIDR (e.g. 10.0.1.0/24), or a prefix length (`24` or `/24`) for
    /// the next free block of that size in the VPC.
    #[arg(long)]
    pub cidr: String,
}

#[derive(Args, Debug)]
pub struct SubnetDelete {
    /// The VPC (name or id).
    pub vpc: String,
    /// The subnet (name or id).
    pub subnet: String,
    /// Do not ask for confirmation.
    #[arg(short = 'y', long)]
    pub yes: bool,
}

// ---- handlers --------------------------------------------------------------------

pub async fn vpc(ctx: &Ctx, cmd: VpcCmd, io: &mut Io<'_>) -> CliResult<()> {
    let k = vpc_kind();
    match cmd {
        VpcCmd::List(a) => commands::do_list(ctx, io, k, a).await,
        VpcCmd::Show(a) => commands::do_show(ctx, io, k, a).await,
        VpcCmd::Delete(a) => commands::do_delete(ctx, io, k, a).await,
        VpcCmd::Create(a) => create(ctx, io, a).await,
        VpcCmd::Subnet { cmd } => subnet(ctx, cmd, io).await,
    }
}

async fn create(ctx: &Ctx, io: &mut Io<'_>, a: VpcCreate) -> CliResult<()> {
    let k = vpc_kind();
    let c = ctx.client()?;
    let external = match &a.external_network {
        Some(net) => Some(commands::resolve(&c, commands::kind("network"), net).await?),
        None => None,
    };
    // A field the caller did not give is left out, not sent as null, so the
    // module's own defaults apply.
    let mut spec = Map::new();
    spec.insert("cidrs".into(), json!(a.cidrs));
    if let Some(net) = external {
        spec.insert("external_network".into(), json!(net));
    }
    if let Some(d) = a.dns_domain.as_deref().map(str::trim) {
        if !d.is_empty() {
            spec.insert("dns_domain".into(), json!(d));
        }
    }
    let body = commands::create_body(k, &a.name, &a.meta, Value::Object(spec))?;
    commands::do_create(ctx, io, k, body, false)
        .await
        .map(|_| ())
}

async fn subnet(ctx: &Ctx, cmd: SubnetCmd, io: &mut Io<'_>) -> CliResult<()> {
    match cmd {
        SubnetCmd::List(a) => subnet_list(ctx, io, a).await,
        SubnetCmd::Create(a) => subnet_create(ctx, io, a).await,
        SubnetCmd::Delete(a) => subnet_delete(ctx, io, a).await,
    }
}

/// `GET /network/vpcs/{vpc}/subnets`, printed with the subnet kind's columns.
async fn subnet_list(ctx: &Ctx, io: &mut Io<'_>, a: VpcArg) -> CliResult<()> {
    let c = ctx.client()?;
    let vpc = commands::resolve(&c, vpc_kind(), &a.vpc).await?;
    let path = format!("{}/{vpc}/subnets", vpc_kind().path);
    let items = list_all(&c, &path).await?;
    print_list(io.stdout, ctx.fmt, subnet_kind(), &items)?;
    Ok(())
}

/// `POST /network/vpcs/{vpc}/subnets` — a network and its one subnet made
/// together. The body is the fabric `VpcSubnet`: name, cidr (a block or a bare
/// prefix length), and the optional gateway, DNS and DHCP, which the CLI does
/// not expose and so leaves for the module's defaults.
async fn subnet_create(ctx: &Ctx, io: &mut Io<'_>, a: SubnetCreate) -> CliResult<()> {
    let c = ctx.client()?;
    let vpc = commands::resolve(&c, vpc_kind(), &a.vpc).await?;
    if a.cidr.trim().is_empty() {
        return Err(CliError::usage("subnet cidr must not be empty"));
    }
    let body = json!({"name": a.name, "cidr": a.cidr});
    let path = format!("{}/{vpc}/subnets", vpc_kind().path);
    let v = c.post(&path, &body).await?;
    commands::report(ctx, io, subnet_kind(), &v, "created")
}

/// `DELETE /network/vpcs/{vpc}/subnets/{subnet}`, after the usual confirmation.
async fn subnet_delete(ctx: &Ctx, io: &mut Io<'_>, a: SubnetDelete) -> CliResult<()> {
    let c = ctx.client()?;
    let vpc = commands::resolve(&c, vpc_kind(), &a.vpc).await?;
    let subnet = commands::resolve(&c, subnet_kind(), &a.subnet).await?;
    if !a.yes && !commands::confirm(io, "delete", &format!("subnet \"{}\"", a.subnet))? {
        writeln!(io.stderr, "aborted")?;
        return Ok(());
    }
    let path = format!("{}/{vpc}/subnets/{subnet}", vpc_kind().path);
    let (status, _) = c.delete(&path).await?;
    let verb = if status == reqwest::StatusCode::ACCEPTED {
        "deletion requested"
    } else {
        "deleted"
    };
    writeln!(io.stdout, "subnet \"{}\" {verb}", a.subnet)?;
    Ok(())
}

/// Every page of a collection path that already carries its own route, paged
/// the way the generic walk pages `/network/subnets`.
async fn list_all(c: &Client, path: &str) -> CliResult<Vec<Value>> {
    let mut items = Vec::new();
    let mut after: Option<String> = None;
    loop {
        let full = match &after {
            Some(a) => format!("{path}?after={a}"),
            None => path.to_owned(),
        };
        let page: pistack_types::Page<Value> = c.get(&full).await?;
        let got = page.items.len();
        items.extend(page.items);
        match page.next {
            Some(n) if got > 0 && Some(&n) != after.as_ref() => after = Some(n),
            _ => break,
        }
    }
    Ok(items)
}
