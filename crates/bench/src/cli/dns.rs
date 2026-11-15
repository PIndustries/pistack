// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! DNS: `pi dns zone` and `pi dns record`, for the zones and record sets the
//! DNS module serves.
//!
//! Everything the noun needs lives here: argument types, handlers and the two
//! column sets. The paths are the DNS module's own: `/dns/zones` and
//! `/dns/recordsets`, each taking `{name, description, labels, spec}` on
//! create and resolving `GET`/`DELETE` by name or id. A record set is also
//! resolved inside one zone, when `--zone` names it: that is where its short
//! `<owner>-<type>` name is read.

use super::args::{DeleteArgs, ListArgs, MetaArgs, NameArg};
use super::client::{CliError, CliResult, Client};
use super::commands::{confirm, create_body, list_all, list_walk};
use super::kinds::{Column, KindDef, UpdateStyle};
use super::output::{self, join, print_list, print_one, scalar, short_id, str_at, Format, Output};
use super::{Ctx, Io};
use clap::{Args, Subcommand};
use serde_json::{json, Map, Value};
use std::collections::HashMap;

// ---- column sets ---------------------------------------------------------------

fn records(v: &Value, _: chrono::DateTime<chrono::Utc>) -> String {
    v.pointer("/spec/records")
        .and_then(Value::as_array)
        .map(|a| join(a))
        .unwrap_or_default()
}

fn zone_ttl(v: &Value, _: chrono::DateTime<chrono::Utc>) -> String {
    match v.pointer("/spec/ttl").and_then(Value::as_u64) {
        Some(n) => n.to_string(),
        None => "-".to_owned(),
    }
}

fn primary(v: &Value, _: chrono::DateTime<chrono::Utc>) -> String {
    if str_at(v, "/spec/kind/type") == "secondary" {
        // The primaries live inside the tagged kind, and are the column's
        // whole point: where this zone is transferred from.
        return v
            .pointer("/spec/kind/primaries")
            .and_then(Value::as_array)
            .map(|a| join(a))
            .unwrap_or_default();
    }
    "primary".to_owned()
}

fn serial(v: &Value, _: chrono::DateTime<chrono::Utc>) -> String {
    v.pointer("/status/serial")
        .and_then(Value::as_u64)
        .map(|n| n.to_string())
        .unwrap_or_else(|| "-".to_owned())
}

/// A zone as a record listing needs it: the TTL a set with none of its own
/// falls back to, and the apex a set's owner sits inside.
pub(crate) struct ZoneInfo {
    pub ttl: String,
    pub apex: String,
}

/// Zone id -> that zone's own TTL and apex, as the record listing reads them
/// back off the zone rows.
pub(crate) type Zones = HashMap<String, ZoneInfo>;

/// The rows a record listing draws. The TTL of a set that does not set one is
/// its zone's, which the row itself does not carry, so the caller hands the
/// zones in and the field is filled before the table is drawn. `-o
/// json|yaml|name` pass the rows through as the server sent them, as
/// everywhere else — a script must not be shown a number the API never said.
fn record_rows(items: &[Value], zones: &Zones) -> Vec<Value> {
    items
        .iter()
        .map(|it| match it.pointer("/spec/ttl") {
            Some(_) => it.clone(),
            None => match zones
                .get(&str_at(it, "/spec/zone"))
                .and_then(|z| z.ttl.parse::<u64>().ok())
            {
                Some(ttl) => {
                    let mut row = it.clone();
                    row["spec"]["ttl"] = json!(ttl);
                    row
                }
                None => it.clone(),
            },
        })
        .collect()
}

/// Render a record set list.
pub(crate) fn print_records(
    out: &mut dyn std::io::Write,
    fmt: Format,
    items: &[Value],
    zones: &Zones,
) -> anyhow::Result<()> {
    if fmt.output != Output::Table && fmt.output != Output::Wide {
        return print_list(out, fmt, &RECORD, items);
    }
    print_list(out, fmt, &RECORD, &record_rows(items, zones))
}

/// Render one record set. `show` gets the same columns `list` gives and the
/// same fallback, so the same set reads the same way in both; the document
/// outputs stay the server's bytes.
pub(crate) fn print_record_one(
    out: &mut dyn std::io::Write,
    fmt: Format,
    item: &Value,
    zones: &Zones,
) -> anyhow::Result<()> {
    match fmt.output {
        Output::Table | Output::Wide => print_list(
            out,
            fmt,
            &RECORD,
            &record_rows(std::slice::from_ref(item), zones),
        ),
        _ => print_one(out, fmt, item),
    }
}

fn record_ttl(v: &Value, _: chrono::DateTime<chrono::Utc>) -> String {
    match v.pointer("/spec/ttl") {
        Some(t) => scalar(t),
        None => "-".to_owned(),
    }
}

fn age(v: &Value, now: chrono::DateTime<chrono::Utc>) -> String {
    output::age(&str_at(v, "/created_at"), now)
}

fn id(v: &Value, _: chrono::DateTime<chrono::Utc>) -> String {
    short_id(&str_at(v, "/id"))
}

fn full_id(v: &Value, _: chrono::DateTime<chrono::Utc>) -> String {
    str_at(v, "/id")
}

fn name(v: &Value, _: chrono::DateTime<chrono::Utc>) -> String {
    str_at(v, "/name")
}

/// The short name a record set is filed under: its owner and type, lower-cased
/// and joined with a dash — `www` A is `www-a`. The name the module gives an
/// automatic set, so a hand-made one and a managed one share it.
fn short_set_name(owner: &str, rtype: &str) -> String {
    format!(
        "{}-{}",
        owner.trim_end_matches('.').to_ascii_lowercase(),
        rtype.to_ascii_lowercase()
    )
}

/// The same name with the zone after it: `www-a.example.cloud`. A resource
/// name is unique within a project, not within a zone, so two zones holding
/// the same host want the same short name and the second create is refused by
/// a name belonging to the other zone's set. The apex carries up to 253
/// characters and a name holds 128, so the short form is what gets cut when the
/// pair does not fit.
fn suffixed_name(short: &str, apex: &str) -> String {
    let apex = apex.trim_end_matches('.').to_ascii_lowercase();
    if apex.is_empty() {
        return short.to_owned();
    }
    let full = format!("{short}.{apex}");
    if full.len() <= 128 {
        return full;
    }
    let mut cut = (128 - apex.len() - 1).min(short.len());
    while !short.is_char_boundary(cut) {
        cut -= 1;
    }
    short[..cut].trim_end_matches('-').to_owned()
}

/// A set as the caller names it under `--zone`: by id, by the name it is filed
/// under, or by its default name — `<owner>-<type>`, or that with the zone's
/// apex after it. The id and the filed name are the store's own; both default
/// forms are built here from the set's owner and type, since the zone is
/// already settled and the row's own name says nothing about it.
fn set_matches(rs: &Value, key: &str, zones: &Zones) -> bool {
    if str_at(rs, "/id") == key || str_at(rs, "/name") == key {
        return true;
    }
    let owner = str_at(rs, "/spec/name");
    if owner.is_empty() {
        return false;
    }
    // The apex belongs to the zone, not the row: a set carries only its id.
    // With dots on both sides — the module keeps the apex absolute — the owner
    // is what is left of the row's name once the zone is taken off its end.
    let apex = zones
        .get(&str_at(rs, "/spec/zone"))
        .map(|z| z.apex.clone())
        .unwrap_or_default();
    let suffix = apex.trim_end_matches('.');
    let bare = match owner
        .trim_end_matches('.')
        .strip_suffix(suffix)
        .and_then(|o| o.strip_suffix('.'))
        .filter(|o| !o.is_empty())
    {
        Some(o) => o.to_owned(),
        _ => owner.trim_end_matches('.').to_owned(),
    };
    let short = short_set_name(&bare, &str_at(rs, "/spec/type"));
    key == short || key == suffixed_name(&short, &apex)
}

fn labels(v: &Value, _: chrono::DateTime<chrono::Utc>) -> String {
    v.pointer("/labels")
        .and_then(Value::as_object)
        .map(|m| {
            m.iter()
                .map(|(k, v)| format!("{k}={}", scalar(v)))
                .collect::<Vec<_>>()
                .join(",")
        })
        .unwrap_or_default()
}

macro_rules! dcol {
    ($h:literal, $f:ident) => {
        Column {
            header: $h,
            get: $f,
        }
    };
    ($h:literal, $ptr:literal) => {
        Column {
            header: $h,
            get: |v, _| str_at(v, $ptr),
        }
    };
    ($h:literal, list $ptr:literal) => {
        Column {
            header: $h,
            get: |v, _| {
                v.pointer($ptr)
                    .and_then(Value::as_array)
                    .map(|a| join(a))
                    .unwrap_or_default()
            },
        }
    };
}

const ZONE_COLUMNS: &[Column] = &[
    dcol!("NAME", name),
    dcol!("ID", id),
    dcol!("PHASE", "/status/phase"),
    dcol!("APEX", "/spec/name"),
    dcol!("TTL", zone_ttl),
    dcol!("EMAIL", "/spec/email"),
    dcol!("SERIAL", serial),
    dcol!("RECORDS", "/status/records"),
    dcol!("AGE", age),
];

const ZONE_WIDE: &[Column] = &[
    dcol!("KIND", "/spec/kind/type"),
    dcol!("NAMESERVERS", list "/spec/nameservers"),
    dcol!("NOTIFY", list "/spec/also_notify"),
    Column {
        header: "PRIMARY",
        get: primary,
    },
    dcol!("FULL-ID", full_id),
    dcol!("LABELS", labels),
];

const RECORD_COLUMNS: &[Column] = &[
    dcol!("NAME", name),
    dcol!("ID", id),
    dcol!("OWNER", "/spec/name"),
    dcol!("TYPE", "/spec/type"),
    dcol!("TTL", record_ttl),
    dcol!("RECORDS", records),
    dcol!("AGE", age),
];

const RECORD_WIDE: &[Column] = &[
    dcol!("ZONE", "/spec/zone"),
    dcol!("PHASE", "/status/phase"),
    dcol!("FULL-ID", full_id),
    dcol!("LABELS", labels),
];

/// An authoritative zone.
pub static ZONE: KindDef = KindDef {
    noun: "zone",
    aliases: &["zones"],
    path: "/dns/zones",
    id_prefix: "zone",
    update: UpdateStyle::Spec,
    flat_create: false,
    columns: ZONE_COLUMNS,
    wide: ZONE_WIDE,
};

pub static RECORD: KindDef = KindDef {
    noun: "record",
    aliases: &["records", "recordset", "recordsets"],
    path: "/dns/recordsets",
    id_prefix: "rec",
    update: UpdateStyle::Spec,
    flat_create: false,
    columns: RECORD_COLUMNS,
    wide: RECORD_WIDE,
};

// ---- argument types -------------------------------------------------------------

#[derive(Subcommand, Debug)]
pub enum DnsCmd {
    /// DNS zones.
    Zone {
        #[command(subcommand)]
        cmd: ZoneCmd,
    },
    /// DNS record sets.
    Record {
        #[command(subcommand)]
        cmd: RecordCmd,
    },
}

#[derive(Subcommand, Debug)]
pub enum ZoneCmd {
    /// List zones.
    #[command(alias = "ls")]
    List(ListArgs),
    /// Show one zone by name, id or apex.
    #[command(alias = "get")]
    Show(NameArg),
    /// Create a zone.
    Create(ZoneCreate),
    /// Delete a zone and every record set in it.
    #[command(alias = "rm")]
    Delete(DeleteArgs),
}

#[derive(Subcommand, Debug)]
pub enum RecordCmd {
    /// List record sets, optionally only one zone's.
    #[command(alias = "ls")]
    List(RecordListArgs),
    /// Show one record set by name or id, optionally within one zone.
    #[command(alias = "get")]
    Show(RecordShowArgs),
    /// Create a record set.
    Create(RecordCreate),
    /// Delete a record set.
    #[command(alias = "rm")]
    Delete(RecordDeleteArgs),
}

#[derive(Args, Debug)]
pub struct ZoneCreate {
    /// Apex of the zone, e.g. example.cloud or example.cloud. A missing final
    /// dot is added.
    pub apex: String,
    /// Name the zone is filed under. Default: the apex without its final dot.
    #[arg(long)]
    pub name: Option<String>,
    #[command(flatten)]
    pub meta: MetaArgs,
    /// Default TTL for records that do not set one, in seconds.
    #[arg(long, default_value_t = 3600)]
    pub ttl: u32,
    /// Contact mailbox for the SOA, e.g. hostmaster@example.cloud.
    #[arg(long)]
    pub email: Option<String>,
    /// Authoritative nameserver (repeatable, absolute name). Default:
    /// ns1.<apex>.
    #[arg(long = "nameserver", value_name = "NAMESERVER")]
    pub nameservers: Vec<String>,
    /// Address of an outside secondary of this zone (`ip` or `ip:port`,
    /// repeatable) so it is told when the serial moves. Primary zones only.
    #[arg(long = "also-notify", value_name = "ADDR")]
    pub also_notify: Vec<String>,
    /// Primary to transfer the zone from (`ip` or `ip:port`, repeatable).
    /// Makes the zone a secondary: served only once a transfer has filled it,
    /// and record sets cannot be added to it.
    #[arg(long = "primary", value_name = "ADDR", conflicts_with_all = ["also_notify"])]
    pub primaries: Vec<String>,
}

#[derive(Args, Debug)]
pub struct RecordListArgs {
    /// Every project (admin).
    #[arg(long)]
    pub all: bool,
    /// Label selector `key=value` (repeatable).
    #[arg(short = 'l', long = "selector", value_name = "K=V")]
    pub selector: Vec<String>,
    /// Only this zone's record sets (name, id or apex).
    #[arg(long, value_name = "ZONE")]
    pub zone: Option<String>,
}

#[derive(Args, Debug)]
pub struct RecordShowArgs {
    /// Name or id.
    pub name: String,
    /// Look for it in this zone only (name, id or apex), the way delete takes
    /// it. Without this a record set is found by its own name or id alone.
    #[arg(long, value_name = "ZONE")]
    pub zone: Option<String>,
}

#[derive(Args, Debug)]
pub struct RecordDeleteArgs {
    /// Name or id.
    pub name: String,
    /// Look for it in this zone only (name, id or apex). The set is then found
    /// among that zone's own, under its short form as well as its full name.
    #[arg(long, value_name = "ZONE")]
    pub zone: Option<String>,
    /// Do not ask for confirmation.
    #[arg(short = 'y', long)]
    pub yes: bool,
    /// Wait until the resource is gone.
    #[arg(long)]
    pub wait: bool,
}

#[derive(Args, Debug)]
pub struct RecordCreate {
    /// Name the record set is filed under. Default: `<owner>-<type>` lower-cased,
    /// with the zone after it if that name is already taken in the project.
    #[arg(long)]
    pub name: Option<String>,
    #[command(flatten)]
    pub meta: MetaArgs,
    /// Owner name, relative to the zone (`web`), absolute with a final dot,
    /// or `@` for the apex.
    pub owner: String,
    /// Record type: A, AAAA, CNAME, MX, TXT, SRV, NS or PTR.
    #[arg(value_name = "TYPE")]
    pub rtype: String,
    /// Zone the record lives in (name, id or apex).
    #[arg(long)]
    pub zone: String,
    /// TTL in seconds. Default: the zone's.
    #[arg(long)]
    pub ttl: Option<u32>,
    /// Record data (repeatable), presentation format: `10.0.0.4`,
    /// `10 mail.example.cloud.`, `"v=spf1 -all"`.
    #[arg(long = "record", value_name = "DATA", required = true)]
    pub records: Vec<String>,
}

/// A zone or record set as a path segment. The server resolves a path by
/// resource name, id or apex, and the names the CLI itself will build — an
/// apex that keeps an inner dot (`copy.example`), an owner, an apex — contain
/// exactly the characters a path is parsed on, so they go out percent-encoded
/// or the server reads `copy.example` as a path and answers `not found` for a
/// zone it just listed.
fn segment(name_or_id: &str) -> String {
    if name_or_id
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~'))
    {
        return name_or_id.to_owned();
    }
    super::commands::query_escape(name_or_id)
}

/// `GET`/`PUT`/`DELETE /dns/<kind>/<name-or-id>`, quoted as [`segment`] says
/// it must be.
fn item_path(k: &KindDef, name_or_id: &str) -> String {
    format!("{}/{}", k.path, segment(name_or_id))
}

// ---- handlers --------------------------------------------------------------------

pub async fn dns(ctx: &Ctx, cmd: DnsCmd, io: &mut Io<'_>) -> CliResult<()> {
    match cmd {
        DnsCmd::Zone { cmd } => zone(ctx, cmd, io).await,
        DnsCmd::Record { cmd } => record(ctx, cmd, io).await,
    }
}

async fn zone(ctx: &Ctx, cmd: ZoneCmd, io: &mut Io<'_>) -> CliResult<()> {
    match cmd {
        ZoneCmd::List(a) => {
            let c = ctx.client()?;
            let items = list_all(&c, &ZONE, &list_args(&a.all, &a.selector)).await?;
            print_list(io.stdout, ctx.fmt, &ZONE, &items)?;
            Ok(())
        }
        ZoneCmd::Show(a) => {
            let c = ctx.client()?;
            let v = c.get_value(&item_path(&ZONE, &a.name)).await?;
            print_one(io.stdout, ctx.fmt, &v)?;
            Ok(())
        }
        ZoneCmd::Create(a) => {
            let apex = a.apex.trim();
            if apex.is_empty() {
                return Err(CliError::usage("zone apex must not be empty"));
            }
            let apex = if apex.ends_with('.') {
                apex.to_owned()
            } else {
                format!("{apex}.")
            };
            // The zone's own name is the apex stripped of its single trailing
            // dot — every other dot is a label boundary, so `copy.example`
            // keeps its middle dot and `example.cloud.` becomes `example.cloud`.
            // Trimming at the first dot would file it as `copy`.
            let name = match a.name.as_deref() {
                Some(n) if !n.trim().is_empty() => n.trim().to_owned(),
                _ => apex.strip_suffix('.').unwrap_or(&apex).to_owned(),
            };
            // ZoneKind is internally tagged, so `kind` is an object naming its
            // type; a secondary names the primaries it pulls from inside it.
            let kind = if a.primaries.is_empty() {
                json!({"type": "primary"})
            } else {
                json!({"type": "secondary", "primaries": a.primaries})
            };
            // A field the caller did not give is left out, not sent as null:
            // the module's own defaults (`email`, and `also_notify` when
            // empty) then apply, and a null would be a type error the module
            // answers with a 422 rather than a default.
            let mut spec = Map::new();
            spec.insert("name".into(), json!(apex));
            spec.insert("kind".into(), kind);
            spec.insert("ttl".into(), json!(a.ttl));
            if let Some(e) = a.email.as_deref().map(str::trim) {
                if !e.is_empty() {
                    spec.insert("email".into(), json!(e));
                }
            }
            if !a.nameservers.is_empty() {
                spec.insert("nameservers".into(), json!(a.nameservers));
            }
            if !a.also_notify.is_empty() {
                spec.insert("also_notify".into(), json!(a.also_notify));
            }
            let body = create_body(&ZONE, &name, &a.meta, Value::Object(spec))?;
            let c = ctx.client()?;
            let v = c.post(ZONE.path, &body).await?;
            report(ctx, io, &ZONE, &v, "created")
        }
        ZoneCmd::Delete(a) => delete(ctx, io, &ZONE, &a).await,
    }
}

async fn record(ctx: &Ctx, cmd: RecordCmd, io: &mut Io<'_>) -> CliResult<()> {
    match cmd {
        RecordCmd::List(a) => {
            let c = ctx.client()?;
            // `--zone` is the module's own filter, so the page comes back
            // already narrowed to one zone's sets.
            let extra = match &a.zone {
                Some(z) => vec![("zone", z.clone())],
                None => Vec::new(),
            };
            let items = list_walk(&c, &RECORD, &list_args(&a.all, &a.selector), &extra).await?;
            let zones = zones_for(&c, &items).await?;
            print_records(io.stdout, ctx.fmt, &items, &zones)?;
            Ok(())
        }
        RecordCmd::Show(a) => {
            let c = ctx.client()?;
            match &a.zone {
                None => {
                    let v = c.get_value(&item_path(&RECORD, &a.name)).await?;
                    let zones = zones_for(&c, std::slice::from_ref(&v)).await?;
                    print_record_one(io.stdout, ctx.fmt, &v, &zones)?;
                }
                Some(z) => {
                    let hits = find_in_zone(&c, z, &a.name).await?;
                    let v = match pick(io, &hits, &a.name, z)? {
                        Some(v) => v,
                        None => {
                            return Err(CliError::usage(format!(
                                "record set '{}' not found in zone '{z}'",
                                a.name
                            )))
                        }
                    };
                    let zones = zones_for(&c, std::slice::from_ref(v)).await?;
                    print_record_one(io.stdout, ctx.fmt, v, &zones)?;
                }
            }
            Ok(())
        }
        RecordCmd::Create(a) => {
            let rtype = a.rtype.trim().to_ascii_uppercase();
            if !RECORD_TYPES.contains(&rtype.as_str()) {
                return Err(CliError::usage(format!(
                    "unknown record type '{}'; use one of {}",
                    a.rtype,
                    RECORD_TYPES.join(", ")
                )));
            }
            let owner = a.owner.trim();
            if owner.is_empty() {
                return Err(CliError::usage("record owner must not be empty"));
            }
            for rec in &a.records {
                if let Some(why) = rdata_problem(&rtype, rec) {
                    return Err(CliError::usage(format!(
                        "{rtype} record '{rec}': {why}; use the presentation format for the type"
                    )));
                }
            }
            let c = ctx.client()?;
            let zone = resolve_zone(&c, &a.zone).await?;
            // No --ttl leaves the field out entirely: `Option<u32>` reads an
            // absent field as "fall back to the zone", and would reject a
            // null outright.
            let mut spec = Map::new();
            spec.insert("zone".into(), json!(zone));
            spec.insert("name".into(), json!(owner));
            // The module's field is `kind` in Rust and `type` on the wire.
            spec.insert("type".into(), json!(rtype));
            if let Some(t) = a.ttl {
                spec.insert("ttl".into(), json!(t));
            }
            spec.insert("records".into(), json!(a.records));
            let spec = Value::Object(spec);
            // A hand-made name is sent as typed, once. The default is tried
            // short first — `<owner>-<type>`, the name the module itself files
            // an automatic set under — and only a name already taken elsewhere
            // in the project gets the zone after it.
            let v = match &a.name {
                Some(n) => {
                    let body = create_body(&RECORD, n, &a.meta, spec)?;
                    c.post(RECORD.path, &body).await?
                }
                None => {
                    let short = short_set_name(owner, &rtype);
                    match c
                        .post(
                            RECORD.path,
                            &create_body(&RECORD, &short, &a.meta, spec.clone())?,
                        )
                        .await
                    {
                        Ok(v) => v,
                        Err(e) if e.is_name_taken(&short) => {
                            let apex = apex_of(&c, &zone).await?;
                            let long = suffixed_name(&short, &apex);
                            let body = create_body(&RECORD, &long, &a.meta, spec)?;
                            c.post(RECORD.path, &body).await?
                        }
                        Err(e) => return Err(e),
                    }
                }
            };
            report(ctx, io, &RECORD, &v, "created")
        }
        RecordCmd::Delete(a) => record_delete(ctx, io, &a).await,
    }
}

const RECORD_TYPES: &[&str] = &["A", "AAAA", "CNAME", "MX", "TXT", "SRV", "NS", "PTR"];

/// Why `rdata` cannot be `{rtype}` data, or `None` when it can. The shapes the
/// help offers: `10.0.0.4`, `2001:db8::1`, `10 mail.example.cloud.`,
/// `"v=spf1 -all"`. Anything the server would refuse is caught here instead,
/// so a mistyped value costs nothing and reads as a usage error.
fn rdata_problem(rtype: &str, rdata: &str) -> Option<&'static str> {
    let r = rdata.trim();
    if r.is_empty() {
        return Some("must not be empty");
    }
    let ok = match rtype {
        "A" => r.parse::<std::net::Ipv4Addr>().is_ok(),
        "AAAA" => r.parse::<std::net::Ipv6Addr>().is_ok(),
        "CNAME" | "NS" | "PTR" => r.ends_with('.'),
        // A data item is `priority host.`, `weight port target.`, or a plain
        // name — the host part is what carries the final dot.
        "MX" => r
            .split_whitespace()
            .last()
            .is_some_and(|h| h.ends_with('.')),
        "SRV" => {
            let parts: Vec<&str> = r.split_whitespace().collect();
            parts.len() == 4
                && parts[..3].iter().all(|p| p.parse::<u16>().is_ok())
                && parts[3].ends_with('.')
        }
        // Text is free form; it is only data once quoted or printed as-is.
        "TXT" => true,
        _ => true,
    };
    if ok {
        None
    } else {
        Some("not valid for this type")
    }
}

/// The zones a record listing touches, read once: the TTL a set with none of
/// its own falls back to, and the apex its short name is measured against.
async fn zones_for(c: &Client, items: &[Value]) -> CliResult<Zones> {
    let mut wanted: Vec<&str> = Vec::new();
    for it in items {
        let z = match it.pointer("/spec/zone").and_then(Value::as_str) {
            Some(z) => z,
            None => continue,
        };
        if !wanted.contains(&z) {
            wanted.push(z);
        }
    }
    let mut out = Zones::new();
    if wanted.is_empty() {
        return Ok(out);
    }
    // One pass over the zones rather than a GET per set: a record listing is
    // exactly the case where a hundred sets share two or three zones.
    for z in list_all(c, &ZONE, &list_args(&false, &[])).await? {
        let id = str_at(&z, "/id");
        if !wanted.contains(&id.as_str()) {
            continue;
        }
        let ttl = match z.pointer("/spec/ttl").and_then(Value::as_u64) {
            Some(ttl) => ttl.to_string(),
            None => String::new(),
        };
        out.insert(
            id,
            ZoneInfo {
                ttl,
                apex: str_at(&z, "/spec/name"),
            },
        );
    }
    Ok(out)
}

/// A zone as its id, so a record set can point at it. The server resolves the
/// path by resource name, id or apex, so anything the caller types works.
async fn resolve_zone(c: &Client, zone: &str) -> CliResult<String> {
    if ZONE.is_id(zone) {
        return Ok(zone.to_owned());
    }
    Ok(str_at(&zone_row(c, zone).await?, "/id"))
}

/// A zone's apex, which its rows do not carry: a record set names its zone by
/// id, and the apex is only in the zone itself.
async fn apex_of(c: &Client, zone: &str) -> CliResult<String> {
    Ok(str_at(&zone_row(c, zone).await?, "/spec/name"))
}

async fn zone_row(c: &Client, zone: &str) -> CliResult<Value> {
    c.get_value(&item_path(&ZONE, zone)).await.map_err(|e| {
        if e.is_not_found() {
            CliError::usage(format!("zone '{zone}' not found"))
        } else {
            e
        }
    })
}

/// The sets in `zone` that answer to `key`. The server resolves a record set
/// path by the set's own name or id, never inside a zone, so the zone's sets
/// are listed and the caller's name picked out of them.
async fn find_in_zone(c: &Client, zone: &str, key: &str) -> CliResult<Vec<Value>> {
    let id = resolve_zone(c, zone).await?;
    let extra = vec![("zone", id.clone())];
    let items = list_walk(c, &RECORD, &list_args(&false, &[]), &extra).await?;
    // The zone's own record, not the page: a row names its zone by id and the
    // apex its owner sits inside lives only on the zone.
    let apex = apex_of(c, &id).await?;
    let mut zones = Zones::new();
    zones.insert(
        id.clone(),
        ZoneInfo {
            ttl: String::new(),
            apex,
        },
    );
    Ok(items
        .into_iter()
        .filter(|it| set_matches(it, key, &zones))
        .collect())
}

/// The one set to act on. A zone-scoped walk can still turn up more than one
/// hit when several sets share a name inside the zone; the first is used and
/// the caller is told, rather than a row being picked silently.
fn pick<'a>(
    io: &mut Io<'_>,
    hits: &'a [Value],
    key: &str,
    zone: &str,
) -> CliResult<Option<&'a Value>> {
    let Some(first) = hits.first() else {
        return Ok(None);
    };
    if hits.len() > 1 {
        writeln!(
            io.stderr,
            "warning: {} record sets in zone '{zone}' answer to '{key}'; using {}",
            hits.len(),
            short_id(&str_at(first, "/id"))
        )?;
    }
    Ok(Some(first))
}

/// The set to act on, as the id to delete and the name to report: found inside
/// `zone` when one is given, by its own name or id otherwise.
async fn record_target(
    io: &mut Io<'_>,
    c: &Client,
    name: &str,
    zone: Option<&str>,
) -> CliResult<(String, String)> {
    let Some(z) = zone else {
        let shown = match c.get_value(&item_path(&RECORD, name)).await {
            Ok(v) if !str_at(&v, "/name").is_empty() => str_at(&v, "/name"),
            Ok(_) => name.to_owned(),
            Err(e) if e.is_not_found() => name.to_owned(),
            Err(e) => return Err(e),
        };
        return Ok((name.to_owned(), shown));
    };
    let hits = find_in_zone(c, z, name).await?;
    match pick(io, &hits, name, z)? {
        Some(v) => Ok((str_at(v, "/id"), str_at(v, "/name"))),
        None => Err(CliError::usage(format!(
            "record set '{name}' not found in zone '{z}'"
        ))),
    }
}

/// `pi dns record delete`, after the usual confirmation. With `--zone` the set
/// is looked up inside that zone and deleted by the id that lookup returned,
/// so the short form cannot be mistaken for a set of the same name elsewhere.
async fn record_delete(ctx: &Ctx, io: &mut Io<'_>, a: &RecordDeleteArgs) -> CliResult<()> {
    let c = ctx.client()?;
    let (target, shown) = record_target(io, &c, &a.name, a.zone.as_deref()).await?;
    delete_by(ctx, io, &RECORD, &target, &shown, a.yes).await
}

/// Delete, after the usual confirmation. `shown` is what the resource is
/// called, so an id given on the command line reads back as a name.
async fn delete(ctx: &Ctx, io: &mut Io<'_>, k: &KindDef, a: &DeleteArgs) -> CliResult<()> {
    let c = ctx.client()?;
    let shown = match c.get_value(&item_path(k, &a.name)).await {
        Ok(v) if !str_at(&v, "/name").is_empty() => str_at(&v, "/name"),
        Ok(_) => a.name.clone(),
        Err(e) if e.is_not_found() => a.name.clone(),
        Err(e) => return Err(e),
    };
    delete_by(ctx, io, k, &a.name, &shown, a.yes).await
}

/// The delete itself: confirm, then remove `target` and report it as `shown`.
async fn delete_by(
    ctx: &Ctx,
    io: &mut Io<'_>,
    k: &KindDef,
    target: &str,
    shown: &str,
    yes: bool,
) -> CliResult<()> {
    let c = ctx.client()?;
    if !yes && !confirm(io, "delete", &format!("{} \"{}\"", k.noun, shown))? {
        return Ok(());
    }
    let (status, _) = c.delete(&item_path(k, target)).await?;
    let verb = if status == reqwest::StatusCode::ACCEPTED {
        "deletion requested"
    } else {
        "deleted"
    };
    writeln!(io.stdout, "{} \"{}\" {verb}", k.noun, shown)?;
    Ok(())
}

/// The shared list walk wants its own argument bag.
fn list_args(all: &bool, selector: &[String]) -> ListArgs {
    ListArgs {
        all: *all,
        selector: selector.to_vec(),
    }
}

/// The CLI's own line for a write: table mode gets a sentence, the rest get
/// the document.
fn report(ctx: &Ctx, io: &mut Io<'_>, k: &KindDef, v: &Value, verb: &str) -> CliResult<()> {
    match ctx.fmt.output {
        Output::Table | Output::Wide => {
            let id = str_at(v, "/id");
            if id.is_empty() {
                writeln!(io.stdout, "{} \"{}\" {verb}", k.noun, str_at(v, "/name"))?;
            } else {
                writeln!(
                    io.stdout,
                    "{} \"{}\" {verb} ({})",
                    k.noun,
                    str_at(v, "/name"),
                    short_id(&id)
                )?;
            }
        }
        Output::Name => writeln!(io.stdout, "{}", str_at(v, "/name"))?,
        _ => print_one(io.stdout, ctx.fmt, v)?,
    }
    Ok(())
}
