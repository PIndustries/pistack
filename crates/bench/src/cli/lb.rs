// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `pi lb`: load balancers, and the listeners, pools and members under them.
//!
//! The relay module mounts these under `/balancers`. A balancer owns a VIP;
//! listeners bind a protocol and port on it, pools hold the backends, and
//! members are one `address:port` each in a pool. Everything here is the CLI
//! side: argument shapes, name-or-id lookups, and the request bodies the
//! module's routes expect.

use super::args::{DeleteArgs, ListArgs, MetaArgs, NameArg};
use super::client::{CliError, CliResult, Client};
use super::commands;
use super::kinds::{Column, KindDef, UpdateStyle};
use super::output::{self, age, print_list, short_id, str_at, Output};
use super::{Ctx, Io};
use clap::{Args, Subcommand, ValueEnum};
use serde_json::{json, Value};
use std::net::IpAddr;

// ---- table columns -----------------------------------------------------------

fn col_name(v: &Value, _: chrono::DateTime<chrono::Utc>) -> String {
    str_at(v, "/name")
}
fn col_id(v: &Value, _: chrono::DateTime<chrono::Utc>) -> String {
    short_id(&str_at(v, "/id"))
}
fn col_phase(v: &Value, _: chrono::DateTime<chrono::Utc>) -> String {
    str_at(v, "/status/phase")
}
fn col_age(v: &Value, now: chrono::DateTime<chrono::Utc>) -> String {
    age(&str_at(v, "/created_at"), now)
}

/// The VIP as realised, with the bound port when one was asked for.
fn col_vip(v: &Value, _: chrono::DateTime<chrono::Utc>) -> String {
    let addr = str_at(v, "/status/vip");
    if addr.is_empty() {
        return String::new();
    }
    match v.pointer("/spec/vip_port").and_then(Value::as_u64) {
        Some(p) if p > 0 => format!("{addr}:{p}"),
        _ => addr,
    }
}

/// `address:port`, with brackets around an IPv6 address.
fn address_port(v: &Value) -> String {
    let addr = str_at(v, "/spec/address");
    if addr.is_empty() {
        return String::new();
    }
    let port = str_at(v, "/spec/port");
    if addr.contains(':') {
        format!("[{addr}]:{port}")
    } else {
        format!("{addr}:{port}")
    }
}

fn col_address(v: &Value, _: chrono::DateTime<chrono::Utc>) -> String {
    address_port(v)
}

fn col_labels(v: &Value, _: chrono::DateTime<chrono::Utc>) -> String {
    v.pointer("/labels")
        .and_then(Value::as_object)
        .map(|m| {
            m.iter()
                .map(|(k, v)| format!("{k}={}", output::scalar(v)))
                .collect::<Vec<_>>()
                .join(",")
        })
        .unwrap_or_default()
}

/// Network ids are useless to read at a glance, so the balancer tables show
/// the name instead. The column lives in a thread local rather than a shared
/// map: it hands out a `fn` pointer that can be stamped into a `&'static
/// Column`, and a poisoned or absent cache simply prints short ids.
mod net_names {
    use std::cell::RefCell;
    use std::collections::HashMap;

    thread_local! {
        static NAMES: RefCell<HashMap<String, String>> = RefCell::new(HashMap::new());
    }

    pub fn remember(id: &str, name: &str) {
        if !id.is_empty() {
            NAMES.with(|n| n.borrow_mut().insert(id.to_owned(), name.to_owned()));
        }
    }

    pub fn name(id: &str) -> Option<String> {
        NAMES.with(|n| {
            let held = n.borrow();
            match held.get(id) {
                Some(name) if !name.is_empty() => Some(name.clone()),
                _ => None,
            }
        })
    }
}

/// The network's name, or its short id when the network cannot be read.
fn col_network(v: &Value, _: chrono::DateTime<chrono::Utc>) -> String {
    let id = str_at(v, "/spec/vip_network");
    if id.is_empty() {
        return String::new();
    }
    net_names::name(&id).unwrap_or_else(|| short_id(&id))
}

fn col_full_id(v: &Value, _: chrono::DateTime<chrono::Utc>) -> String {
    str_at(v, "/id")
}
fn col_project(v: &Value, _: chrono::DateTime<chrono::Utc>) -> String {
    short_id(&str_at(v, "/project"))
}

/// Columns that read one JSON pointer. `const` of a function-pointer type
/// accepts a closure, so each entry costs one line and nothing at runtime.
macro_rules! columns {
    ($($n:ident: $p:literal),* $(,)?) => {
        $(
            const $n: fn(&Value, chrono::DateTime<chrono::Utc>) -> String =
                |v, _| str_at(v, $p);
        )*
    };
}

macro_rules! short_columns {
    ($($n:ident: $p:literal),* $(,)?) => {
        $(
            const $n: fn(&Value, chrono::DateTime<chrono::Utc>) -> String =
                |v, _| short_id(&str_at(v, $p));
        )*
    };
}

columns! {
    COL_AT_SPEC_ALGORITHM: "/spec/algorithm",
    COL_AT_SPEC_BACKUP: "/spec/backup",
    COL_AT_SPEC_PORT: "/spec/port",
    COL_AT_SPEC_PROTOCOL: "/spec/protocol",
    COL_AT_SPEC_PROTOCOL_TYPE: "/spec/protocol/type",
    COL_AT_SPEC_PROVIDER: "/spec/provider",
    COL_AT_SPEC_WEIGHT: "/spec/weight",
    COL_AT_STATUS_MEMBERS: "/status/members",
    COL_AT_STATUS_ONLINE: "/status/online",
    COL_AT_STATUS_OPERATING_STATUS: "/status/operating_status",
}

short_columns! {
    COL_SHORT_SPEC_DEFAULT_POOL: "/spec/default_pool",
}

const WIDE: &[Column] = &[
    Column {
        header: "FULL-ID",
        get: col_full_id,
    },
    Column {
        header: "PROJECT",
        get: col_project,
    },
    Column {
        header: "LABELS",
        get: col_labels,
    },
];

// Kind table entries, built here so `lb` adds no rows to `kinds::KINDS`. Once
// built they live as long as the program, matching `KindDef`'s `&'static`
// columns.
fn kind_balancer() -> &'static KindDef {
    static COLS: &[Column] = &[
        Column {
            header: "NAME",
            get: col_name,
        },
        Column {
            header: "ID",
            get: col_id,
        },
        Column {
            header: "PHASE",
            get: col_phase,
        },
        Column {
            header: "VIP",
            get: col_vip,
        },
        Column {
            header: "NETWORK",
            get: col_network,
        },
        Column {
            header: "PROVIDER",
            get: COL_AT_SPEC_PROVIDER,
        },
        Column {
            header: "AGE",
            get: col_age,
        },
    ];
    static K: std::sync::OnceLock<KindDef> = std::sync::OnceLock::new();
    K.get_or_init(|| KindDef {
        noun: "lb",
        aliases: &["balancer", "balancers", "lbs"],
        path: "/balancers/balancers",
        id_prefix: "lb",
        update: UpdateStyle::Spec,
        flat_create: false,
        columns: COLS,
        wide: WIDE,
    })
}

fn kind_listener() -> &'static KindDef {
    static COLS: &[Column] = &[
        Column {
            header: "NAME",
            get: col_name,
        },
        Column {
            header: "ID",
            get: col_id,
        },
        Column {
            header: "PHASE",
            get: col_phase,
        },
        Column {
            header: "PROTOCOL",
            get: COL_AT_SPEC_PROTOCOL_TYPE,
        },
        Column {
            header: "PORT",
            get: COL_AT_SPEC_PORT,
        },
        Column {
            header: "POOL",
            get: COL_SHORT_SPEC_DEFAULT_POOL,
        },
        Column {
            header: "AGE",
            get: col_age,
        },
    ];
    static K: std::sync::OnceLock<KindDef> = std::sync::OnceLock::new();
    K.get_or_init(|| KindDef {
        noun: "lb-listener",
        aliases: &["listener", "listeners", "lsn"],
        path: "/balancers/listeners",
        id_prefix: "lsn",
        update: UpdateStyle::Spec,
        flat_create: false,
        columns: COLS,
        wide: WIDE,
    })
}

fn kind_pool() -> &'static KindDef {
    static COLS: &[Column] = &[
        Column {
            header: "NAME",
            get: col_name,
        },
        Column {
            header: "ID",
            get: col_id,
        },
        Column {
            header: "PHASE",
            get: col_phase,
        },
        Column {
            header: "PROTOCOL",
            get: COL_AT_SPEC_PROTOCOL,
        },
        Column {
            header: "ALGORITHM",
            get: COL_AT_SPEC_ALGORITHM,
        },
        Column {
            header: "ONLINE",
            get: COL_AT_STATUS_ONLINE,
        },
        Column {
            header: "MEMBERS",
            get: COL_AT_STATUS_MEMBERS,
        },
        Column {
            header: "AGE",
            get: col_age,
        },
    ];
    static K: std::sync::OnceLock<KindDef> = std::sync::OnceLock::new();
    K.get_or_init(|| KindDef {
        noun: "lb-pool",
        aliases: &["pool", "pools"],
        path: "/balancers/pools",
        id_prefix: "pool",
        update: UpdateStyle::Spec,
        flat_create: false,
        columns: COLS,
        wide: WIDE,
    })
}

fn kind_member() -> &'static KindDef {
    static COLS: &[Column] = &[
        Column {
            header: "NAME",
            get: col_name,
        },
        Column {
            header: "ID",
            get: col_id,
        },
        Column {
            header: "ADDRESS",
            get: col_address,
        },
        Column {
            header: "WEIGHT",
            get: COL_AT_SPEC_WEIGHT,
        },
        Column {
            header: "BACKUP",
            get: COL_AT_SPEC_BACKUP,
        },
        Column {
            header: "STATUS",
            get: COL_AT_STATUS_OPERATING_STATUS,
        },
        Column {
            header: "AGE",
            get: col_age,
        },
    ];
    static K: std::sync::OnceLock<KindDef> = std::sync::OnceLock::new();
    K.get_or_init(|| KindDef {
        noun: "lb-member",
        aliases: &["member", "members", "mbr"],
        path: "/balancers/members",
        id_prefix: "mbr",
        update: UpdateStyle::Spec,
        flat_create: false,
        columns: COLS,
        wide: WIDE,
    })
}

// ---- argument types ----------------------------------------------------------

#[derive(Clone, Copy, Debug, ValueEnum, Default)]
pub enum LbProviderArg {
    /// L4 TCP and UDP. The default.
    #[default]
    Forward,
    /// HTTP, terminated at the balancer.
    Relay,
}

#[derive(Args, Debug)]
pub struct LbCreate {
    pub name: String,
    #[command(flatten)]
    pub meta: MetaArgs,
    /// Network (name or id) the VIP lives on.
    #[arg(long)]
    pub network: String,
    /// Explicit VIP address (default: allocated).
    #[arg(long)]
    pub vip: Option<String>,
    /// Bind this exact port on the VIP instead of an ephemeral one.
    #[arg(long)]
    pub vip_port: Option<u16>,
    /// Data plane.
    #[arg(long, value_enum, default_value_t = LbProviderArg::Forward)]
    pub provider: LbProviderArg,
    /// Bring it up administratively off.
    #[arg(long)]
    pub down: bool,
}

/// Scope shared by the sub-resource lists.
#[derive(Args, Debug)]
pub struct LbScope {
    /// Every project (admin).
    #[arg(long)]
    pub all: bool,
    /// Filter to one balancer (name or id).
    #[arg(long)]
    pub balancer: Option<String>,
    /// Filter to one pool (name or id); members only.
    #[arg(long)]
    pub pool: Option<String>,
    /// Members whose name or address contains this (members only).
    #[arg(long)]
    pub only: Option<String>,
    /// Label selector `key=value` (repeatable).
    #[arg(short = 'l', long = "selector", value_name = "K=V")]
    pub selector: Vec<String>,
}

impl From<LbScope> for ListArgs {
    fn from(s: LbScope) -> ListArgs {
        ListArgs {
            all: s.all,
            selector: s.selector,
        }
    }
}

#[derive(Clone, Copy, Debug, ValueEnum, Default)]
pub enum ListenerProto {
    #[default]
    Tcp,
    Udp,
    Http,
    /// HTTPS passed through untouched (L4).
    Https,
    /// TLS terminated at the balancer with a certificate.
    Terminated,
}

#[derive(Args, Debug)]
pub struct ListenerCreate {
    /// Balancer (name or id) the listener binds on.
    #[arg(long)]
    pub balancer: String,
    pub name: String,
    #[command(flatten)]
    pub meta: MetaArgs,
    #[arg(long, value_enum, default_value_t = ListenerProto::Tcp)]
    pub protocol: ListenerProto,
    #[arg(long)]
    pub port: u16,
    /// Default pool (name or id).
    #[arg(long)]
    pub pool: Option<String>,
    /// Certificate id for `--protocol terminated`.
    #[arg(long)]
    pub cert: Option<String>,
    /// Max concurrent connections; 0 means unlimited.
    #[arg(long, default_value_t = 0)]
    pub connection_limit: u32,
    /// Bring it up administratively off.
    #[arg(long)]
    pub down: bool,
}

#[derive(Clone, Copy, Debug, ValueEnum, Default)]
pub enum PoolProto {
    #[default]
    Tcp,
    Udp,
    Http,
    Https,
}

#[derive(Clone, Copy, Debug, Default)]
pub enum PoolAlgorithm {
    #[default]
    RoundRobin,
    LeastConn,
    SourceIp,
}

impl PoolAlgorithm {
    /// The names a pool spec carries, and the only ones `--algorithm`
    /// accepts, so the CLI refuses what the relay would refuse.
    pub const NAMES: [&'static str; 3] = ["round_robin", "least_conn", "source_ip"];

    /// Refused before the request goes out, in the relay's own words.
    pub fn parse(value: &str) -> Result<Self, String> {
        match value.trim() {
            "round_robin" => Ok(Self::RoundRobin),
            "least_conn" => Ok(Self::LeastConn),
            "source_ip" => Ok(Self::SourceIp),
            other => Err(format!(
                "algorithm '{other}' is not a balancing algorithm; use one of {}",
                Self::NAMES.join(", ")
            )),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::RoundRobin => "round_robin",
            Self::LeastConn => "least_conn",
            Self::SourceIp => "source_ip",
        }
    }
}

#[derive(Clone, Copy, Debug, ValueEnum, Default)]
pub enum MonitorKindArg {
    /// A bare connect to the member's port. The default.
    #[default]
    Tcp,
    Http,
    Https,
}

#[derive(Args, Debug)]
pub struct PoolCreate {
    /// Balancer (name or id) the pool belongs to.
    #[arg(long)]
    pub balancer: String,
    pub name: String,
    #[command(flatten)]
    pub meta: MetaArgs,
    #[arg(long, value_enum, default_value_t = PoolProto::Tcp)]
    pub protocol: PoolProto,
    /// Spreading algorithm: round_robin, least_conn or source_ip.
    #[arg(long, default_value = "round_robin", value_name = "ALGORITHM")]
    pub algorithm: String,
    /// Add a health monitor with these probe settings.
    #[arg(long)]
    pub health_check: bool,
    /// What the monitor speaks. Default: the pool protocol; a udp pool is
    /// probed with a bare connect.
    #[arg(long, value_enum, default_value_t = MonitorKindArg::Tcp)]
    pub monitor: MonitorKindArg,
    /// Seconds between probes.
    #[arg(long, default_value_t = 5)]
    pub interval: u32,
    /// Seconds before a probe fails.
    #[arg(long, default_value_t = 3)]
    pub timeout: u32,
    /// Consecutive successes before a member is online.
    #[arg(long, default_value_t = 2)]
    pub rise: u32,
    /// Consecutive failures before a member is offline.
    #[arg(long, default_value_t = 3)]
    pub fall: u32,
    /// Bring it up administratively off.
    #[arg(long)]
    pub down: bool,
}

#[derive(Args, Debug)]
pub struct MemberAdd {
    /// Pool (name or id) to add the backend to.
    #[arg(long)]
    pub pool: String,
    /// Backend address.
    pub address: String,
    /// Backend port.
    #[arg(long)]
    pub port: u16,
    /// Share of traffic; 0-256.
    #[arg(long, default_value_t = 1)]
    pub weight: u16,
    /// Take no new traffic while others are up.
    #[arg(long)]
    pub backup: bool,
    /// Add it administratively off.
    #[arg(long)]
    pub down: bool,
}

#[derive(Args, Debug)]
pub struct MemberRemove {
    /// Pool (name or id).
    #[arg(long)]
    pub pool: String,
    /// Member name, id, `address:port`, or address alone.
    pub member: String,
    /// Do not ask for confirmation.
    #[arg(short = 'y', long)]
    pub yes: bool,
}

#[derive(Subcommand, Debug)]
pub enum ListenerCmd {
    /// List listeners.
    #[command(alias = "ls")]
    List(LbScope),
    /// Show one by name or id.
    #[command(alias = "get")]
    Show(NameArg),
    /// Create a listener on a balancer.
    Create(ListenerCreate),
    /// Delete by name or id.
    #[command(alias = "rm")]
    Delete(DeleteArgs),
}

#[derive(Subcommand, Debug)]
pub enum PoolCmd {
    /// List pools.
    #[command(alias = "ls")]
    List(LbScope),
    /// Show one by name or id.
    #[command(alias = "get")]
    Show(NameArg),
    /// Create a pool on a balancer.
    Create(PoolCreate),
    /// Delete by name or id.
    #[command(alias = "rm")]
    Delete(DeleteArgs),
}

#[derive(Subcommand, Debug)]
pub enum MemberCmd {
    /// List members.
    #[command(alias = "ls")]
    List(LbScope),
    /// Add a backend to a pool.
    Add(MemberAdd),
    /// Remove a backend from a pool.
    #[command(alias = "rm")]
    Remove(MemberRemove),
}

#[derive(Subcommand, Debug)]
pub enum LbCmd {
    /// List balancers.
    #[command(alias = "ls")]
    List(ListArgs),
    /// Show one balancer by name or id.
    #[command(alias = "get")]
    Show(NameArg),
    /// Create a balancer.
    Create(LbCreate),
    /// Delete a balancer and everything under it.
    #[command(alias = "rm")]
    Delete(DeleteArgs),
    /// Listeners on a balancer.
    Listener {
        #[command(subcommand)]
        cmd: ListenerCmd,
    },
    /// Pools on a balancer.
    Pool {
        #[command(subcommand)]
        cmd: PoolCmd,
    },
    /// Members of a pool.
    Member {
        #[command(subcommand)]
        cmd: MemberCmd,
    },
}

// ---- entry point -------------------------------------------------------------

pub async fn dispatch(ctx: &mut Ctx, cmd: LbCmd, io: &mut Io<'_>) -> CliResult<()> {
    match cmd {
        LbCmd::List(a) => {
            let c = ctx.client()?;
            let items = balancers(ctx, &a).await?;
            load_network_names(&c, ctx.fmt.output, &items).await;
            print_list(io.stdout, ctx.fmt, kind_balancer(), &items)?;
            Ok(())
        }
        LbCmd::Show(a) => commands::do_show(ctx, io, kind_balancer(), a).await,
        LbCmd::Delete(a) => commands::do_delete(ctx, io, kind_balancer(), a).await,
        LbCmd::Create(a) => create(ctx, io, a).await,
        LbCmd::Listener { cmd } => listener(ctx, cmd, io).await,
        LbCmd::Pool { cmd } => pool(ctx, cmd, io).await,
        LbCmd::Member { cmd } => member(ctx, cmd, io).await,
    }
}

/// Fill the network-name cache before a balancer table is printed, so a
/// network seen on several balancers is read once. Reads are best-effort: a
/// network nobody can see prints as its short id, and a miss is remembered as
/// one so it is not asked for again. Only `list` reads it; `show` and the
/// machine formats print the stored id.
async fn load_network_names(c: &Client, out: Output, rows: &[Value]) {
    if !matches!(out, Output::Table | Output::Wide) {
        return;
    }
    let net = commands::kind("network");
    let mut ids: Vec<String> = Vec::new();
    for r in rows {
        let id = str_at(r, "/spec/vip_network");
        if !id.is_empty() && net_names::name(&id).is_none() && !ids.contains(&id) {
            ids.push(id);
        }
    }
    for id in ids {
        let name = match c.get_value(&net.item_path(&id)).await {
            Ok(v) => str_at(&v, "/name"),
            Err(_) => String::new(),
        };
        net_names::remember(&id, &name);
    }
}

/// Balancers, paged, the way `commands::do_list` would fetch them.
async fn balancers(ctx: &Ctx, a: &ListArgs) -> CliResult<Vec<Value>> {
    let mut query: Vec<String> = Vec::new();
    if a.all {
        query.push("all=true".into());
    }
    if !a.selector.is_empty() {
        for s in &a.selector {
            if !s.contains('=') {
                return Err(CliError::usage(format!(
                    "selector must be key=value, got '{s}'"
                )));
            }
        }
        query.push(format!("labels={}", a.selector.join(",")));
    }
    let c = ctx.client()?;
    list_paged(&c, kind_balancer().path, &query).await
}

async fn create(ctx: &Ctx, io: &mut Io<'_>, a: LbCreate) -> CliResult<()> {
    let k = kind_balancer();
    let c = ctx.client()?;
    let spec = json!({
        "vip_network": resolve_network(&c, &a.network).await?,
        "vip_address": parse_vip(a.vip.as_deref())?,
        "vip_port": a.vip_port,
        "provider": match a.provider {
            LbProviderArg::Forward => "forward",
            LbProviderArg::Relay => "relay",
        },
        "admin_up": !a.down,
    });
    let body = commands::create_body(k, &a.name, &a.meta, spec)?;
    commands::do_create(ctx, io, k, body, false)
        .await
        .map(|_| ())
}

fn parse_vip(v: Option<&str>) -> CliResult<Value> {
    match v {
        None => Ok(Value::Null),
        Some(s) => {
            let addr: IpAddr = s
                .parse()
                .map_err(|_| CliError::usage(format!("--vip '{s}' is not an IP address")))?;
            Ok(json!(addr))
        }
    }
}

async fn resolve_network(c: &Client, name_or_id: &str) -> CliResult<String> {
    commands::resolve(c, commands::kind("network"), name_or_id).await
}

// ---- listeners ---------------------------------------------------------------

async fn listener(ctx: &Ctx, cmd: ListenerCmd, io: &mut Io<'_>) -> CliResult<()> {
    let k = kind_listener();
    match cmd {
        ListenerCmd::List(scope) => scoped_list(ctx, io, k, scope).await,
        ListenerCmd::Show(a) => commands::do_show(ctx, io, k, a).await,
        ListenerCmd::Delete(a) => commands::do_delete(ctx, io, k, a).await,
        ListenerCmd::Create(a) => listener_create(ctx, io, k, a).await,
    }
}

async fn listener_create(
    ctx: &Ctx,
    io: &mut Io<'_>,
    k: &KindDef,
    a: ListenerCreate,
) -> CliResult<()> {
    let c = ctx.client()?;
    let protocol = match a.protocol {
        ListenerProto::Tcp => json!({"type": "tcp"}),
        ListenerProto::Udp => json!({"type": "udp"}),
        ListenerProto::Http => json!({"type": "http"}),
        ListenerProto::Https => json!({"type": "https"}),
        ListenerProto::Terminated => {
            let cert = a.cert.clone().ok_or_else(|| {
                CliError::usage("--protocol terminated needs --cert <CERTIFICATE>")
            })?;
            json!({"type": "terminated", "cert": cert})
        }
    };
    let spec = json!({
        "balancer": commands::resolve(&c, kind_balancer(), &a.balancer).await?,
        "protocol": protocol,
        "port": a.port,
        "default_pool": commands::resolve_opt(&c, kind_pool(), &a.pool).await?,
        "connection_limit": a.connection_limit,
        "admin_up": !a.down,
    });
    let body = commands::create_body(k, &a.name, &a.meta, spec)?;
    commands::do_create(ctx, io, k, body, false)
        .await
        .map(|_| ())
}

// ---- pools -------------------------------------------------------------------

async fn pool(ctx: &Ctx, cmd: PoolCmd, io: &mut Io<'_>) -> CliResult<()> {
    let k = kind_pool();
    match cmd {
        PoolCmd::List(scope) => scoped_list(ctx, io, k, scope).await,
        PoolCmd::Show(a) => commands::do_show(ctx, io, k, a).await,
        PoolCmd::Delete(a) => commands::do_delete(ctx, io, k, a).await,
        PoolCmd::Create(a) => pool_create(ctx, io, k, a).await,
    }
}

async fn pool_create(ctx: &Ctx, io: &mut Io<'_>, k: &KindDef, a: PoolCreate) -> CliResult<()> {
    let algorithm = PoolAlgorithm::parse(&a.algorithm)
        .map_err(|reason| CliError::usage(format!("--algorithm: {reason}")))?;
    let c = ctx.client()?;
    let protocol = match a.protocol {
        PoolProto::Tcp => "tcp",
        PoolProto::Udp => "udp",
        PoolProto::Http => "http",
        PoolProto::Https => "https",
    };
    // A monitor that does not name its protocol speaks the pool's own; a
    // udp pool is probed with a bare connect.
    let monitor_kind = match (a.monitor, a.protocol) {
        (MonitorKindArg::Tcp, PoolProto::Http) => "http",
        (MonitorKindArg::Tcp, PoolProto::Https) => "https",
        (MonitorKindArg::Tcp, _) => "tcp",
        (MonitorKindArg::Http, _) => "http",
        (MonitorKindArg::Https, _) => "https",
    };
    let health_monitor = if a.health_check {
        json!({
            "kind": monitor_kind,
            "interval": a.interval,
            "timeout": a.timeout,
            "rise": a.rise,
            "fall": a.fall,
        })
    } else {
        Value::Null
    };
    let spec = json!({
        "balancer": commands::resolve(&c, kind_balancer(), &a.balancer).await?,
        "protocol": protocol,
        "algorithm": algorithm.name(),
        "health_monitor": health_monitor,
        "admin_up": !a.down,
    });
    let body = commands::create_body(k, &a.name, &a.meta, spec)?;
    commands::do_create(ctx, io, k, body, false)
        .await
        .map(|_| ())
}

// ---- members -----------------------------------------------------------------

async fn member(ctx: &Ctx, cmd: MemberCmd, io: &mut Io<'_>) -> CliResult<()> {
    let k = kind_member();
    match cmd {
        MemberCmd::List(scope) => scoped_list(ctx, io, k, scope).await,
        MemberCmd::Add(a) => member_add(ctx, io, a).await,
        MemberCmd::Remove(a) => member_remove(ctx, io, a).await,
    }
}

async fn member_add(ctx: &Ctx, io: &mut Io<'_>, a: MemberAdd) -> CliResult<()> {
    let k = kind_member();
    let c = ctx.client()?;
    let pool_id = commands::resolve(&c, kind_pool(), &a.pool).await?;
    let address: IpAddr = a.address.parse().map_err(|_| {
        CliError::usage(format!(
            "backend address '{}' is not an IP address",
            a.address
        ))
    })?;
    if a.weight > 256 {
        return Err(CliError::usage("--weight must be 0-256"));
    }
    let body = json!({
        "address": address,
        "port": a.port,
        "weight": a.weight,
        "backup": a.backup,
        "admin_up": !a.down,
    });
    let path = format!("/balancers/pools/{pool_id}/members");
    let v = c.post(&path, &body).await?;
    commands::report(ctx, io, k, &v, "added")
}

async fn member_remove(ctx: &Ctx, io: &mut Io<'_>, a: MemberRemove) -> CliResult<()> {
    let c = ctx.client()?;
    let pool_id = commands::resolve(&c, kind_pool(), &a.pool).await?;
    let member_id = resolve_member(&c, &pool_id, &a.member).await?;
    if !a.yes && !commands::confirm(io, "remove", &format!("member \"{}\"", a.member))? {
        writeln!(io.stderr, "aborted")?;
        return Ok(());
    }
    let (status, _) = c
        .delete(&format!("/balancers/pools/{pool_id}/members/{member_id}"))
        .await?;
    let verb = if status == reqwest::StatusCode::ACCEPTED {
        "removal requested"
    } else {
        "removed"
    };
    writeln!(io.stdout, "member \"{}\" {verb}", a.member)?;
    Ok(())
}

/// A member under a pool, by name, id, `address:port`, or address alone.
/// The flat `/balancers/members` list filtered by pool keeps every lookup
/// inside the pool it belongs to. An address on its own is only enough when
/// one member of the pool answers to it; otherwise the candidates are
/// listed and the port asked for.
async fn resolve_member(c: &Client, pool_id: &str, term: &str) -> CliResult<String> {
    if kind_member().is_id(term) {
        return Ok(term.to_owned());
    }
    let members = pool_members(c, pool_id).await?;
    let by_field = |p: &str| {
        members
            .iter()
            .find(|m| str_at(m, p) == term)
            .map(|m| str_at(m, "/id"))
    };
    if let Some(id) = by_field("/name").or_else(|| by_field("/id")) {
        return Ok(id);
    }
    if members.is_empty() {
        return Err(CliError::usage(format!("member '{term}' not found")));
    }
    // A short id is what every table prints, so it is what gets typed back.
    if let Some(m) = members.iter().find(|m| short_id(&str_at(m, "/id")) == term) {
        return Ok(str_at(m, "/id"));
    }
    // Brackets say where an IPv6 ends and its port begins; a bare address is
    // matched whole, which is all an IPv6 can be — its own colons leave
    // nothing to split on. Anything else may end in `:port`.
    let (addr, port) = split_endpoint(term);
    let matches: Vec<&Value> = members
        .iter()
        .filter(|m| str_at(m, "/spec/address") == addr)
        .filter(|m| port.is_none_or(|want| str_at(m, "/spec/port") == want.to_string()))
        .collect();
    match matches.len() {
        0 => Err(CliError::usage(format!("member '{term}' not found"))),
        1 => Ok(str_at(matches[0], "/id")),
        _ => {
            let list = matches
                .iter()
                .map(|m| address_port(m))
                .collect::<Vec<_>>()
                .join(", ");
            Err(CliError::usage(format!(
                "'{term}' is several members of the pool ({list}); name one as ADDRESS:PORT"
            )))
        }
    }
}

/// One endpoint into address and port, the address returned as the stack
/// writes it so how it was typed does not decide whether it matches what the
/// server realised. Anything that is not an endpoint comes back whole, which
/// covers a name nobody typed.
fn split_endpoint(s: &str) -> (String, Option<u16>) {
    // Bracketed first: `[fd00::3]:80` leaves nothing to guess at.
    if let Some((addr, port)) = s.strip_prefix('[').and_then(|r| r.split_once("]:")) {
        if let (Some(a), Some(p)) = (address(addr), port.parse::<u16>().ok()) {
            return (a, Some(p));
        }
    }
    // A whole address is the address and no port. Checked before any split,
    // because a bare IPv6 carries colons of its own: `fd00::3:80` is an
    // address, not `fd00::3` on port 80.
    if let Some(a) = address(s) {
        return (a, None);
    }
    // Anything else may end in `:port`, if the rest is an address. A name
    // that failed to match stays one whole term however many colons it holds.
    if let Some((addr, port)) = s.rsplit_once(':') {
        if let (Some(a), Some(p)) = (address(addr), port.parse::<u16>().ok()) {
            return (a, Some(p));
        }
    }
    (s.to_owned(), None)
}

/// An address as the stack writes it, or nothing.
fn address(part: &str) -> Option<String> {
    part.parse::<IpAddr>().ok().map(|a| a.to_string())
}

/// Every member of one pool, past the page bounds.
async fn pool_members(c: &Client, pool_id: &str) -> CliResult<Vec<Value>> {
    list_paged(c, "/balancers/members", &[format!("pool={pool_id}")]).await
}

// ---- list plumbing -----------------------------------------------------------

async fn scoped_list(ctx: &Ctx, io: &mut Io<'_>, k: &KindDef, scope: LbScope) -> CliResult<()> {
    let mut query: Vec<String> = Vec::new();
    if scope.all {
        query.push("all=true".into());
    }
    if let Some(b) = &scope.balancer {
        let c = ctx.client()?;
        query.push(format!(
            "balancer={}",
            commands::resolve(&c, kind_balancer(), b).await?
        ));
    }
    if let Some(p) = &scope.pool {
        let c = ctx.client()?;
        query.push(format!(
            "pool={}",
            commands::resolve(&c, kind_pool(), p).await?
        ));
    }
    // A pool's members have generated names nobody types; `--only` keeps the
    // rows whose name or address contains the term.
    let filter = if k.noun == "lb-member" {
        scope.only.clone()
    } else if scope.only.is_some() {
        return Err(CliError::usage("--only lists the members of one pool"));
    } else {
        None
    };
    let list_args: ListArgs = scope.into();
    if !list_args.selector.is_empty() {
        for s in &list_args.selector {
            if !s.contains('=') {
                return Err(CliError::usage(format!(
                    "selector must be key=value, got '{s}'"
                )));
            }
        }
        query.push(format!("labels={}", list_args.selector.join(",")));
    }
    let c = ctx.client()?;
    let mut items = list_paged(&c, k.path, &query).await?;
    if let Some(term) = filter {
        items.retain(|m| str_at(m, "/name").contains(&term) || address_port(m).contains(&term));
    }
    print_list(io.stdout, ctx.fmt, k, &items)?;
    Ok(())
}

/// Page through a collection path already carrying its query string.
async fn list_paged(c: &Client, path: &str, query: &[String]) -> CliResult<Vec<Value>> {
    let mut items = Vec::new();
    let mut after: Option<String> = None;
    loop {
        let mut q = query.to_vec();
        if let Some(a) = &after {
            q.push(format!("after={a}"));
        }
        let full = if q.is_empty() {
            path.to_owned()
        } else {
            format!("{path}?{}", q.join("&"))
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

#[cfg(test)]
mod tests {
    use super::split_endpoint;

    /// What the CLI has to make of the strings a person types back from a
    /// table, before any of them reaches the server.
    fn endpoint(s: &str) -> (String, Option<u16>) {
        split_endpoint(s)
    }

    #[test]
    fn endpoints_split_where_the_address_ends() {
        assert_eq!(endpoint("10.60.0.3"), ("10.60.0.3".into(), None));
        assert_eq!(endpoint("10.60.0.3:80"), ("10.60.0.3".into(), Some(80)));
        // A bracketed v6 closes before the port, as it is written.
        assert_eq!(endpoint("[fd00::3]:8080"), ("fd00::3".into(), Some(8080)));
        // A bare v6 is all address: `fd00::3:80` is a real address of its own.
        assert_eq!(endpoint("fd00::3"), ("fd00::3".into(), None));
        assert_eq!(endpoint("fd00::3:80"), ("fd00::3:80".into(), None));
        // A name that did not match stays a name, colons and all.
        assert_eq!(
            endpoint("web-pool-10.60.0.3-80"),
            ("web-pool-10.60.0.3-80".into(), None)
        );
        assert_eq!(endpoint("a:b:c"), ("a:b:c".into(), None));
        // An odd port is not a port.
        assert_eq!(
            endpoint("10.60.0.3:99999"),
            ("10.60.0.3:99999".into(), None)
        );
    }

    #[test]
    fn an_address_is_read_the_way_the_stack_writes_it() {
        assert_eq!(endpoint("FD00::0003"), endpoint("fd00::3"));
        assert_eq!(endpoint("[FD00::0003]:80"), endpoint("[fd00::3]:80"));
        // A padded octet is not an address to the stack, so it is not one
        // here either: no member is looked for under it.
        assert_eq!(endpoint("10.60.0.03:80"), ("10.60.0.03:80".into(), None));
    }
}
