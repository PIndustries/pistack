// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `pi alarm` and `pi metric`: threshold alarms in the telemetry service,
//! and the last points a server holds for a series.

use super::args::{DeleteArgs, ListArgs, MetaArgs};
use super::client::{CliError, CliResult, Client};
use super::commands;
use super::kinds::{Column, KindDef, UpdateStyle};
use super::output::{self, print_list, print_one, print_value, short_id, str_at};
use super::{Ctx, Io};
use chrono::{DateTime, Utc};
use clap::{Args, Subcommand, ValueEnum};
use serde_json::{json, Map, Value};

/// How an alarm compares the latest sample with the threshold.
#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum Comparison {
    /// Fires while the value is above it.
    Gt,
    /// Fires while the value is below it.
    Lt,
    /// Fires while the value equals it.
    Eq,
}

/// A threshold the server would accept: a finite number.
fn finite(s: &str) -> Result<f64, String> {
    let v: f64 = s
        .parse()
        .map_err(|_| format!("'{s}' is not a number; the threshold is a value to compare with"))?;
    if v.is_finite() {
        Ok(v)
    } else {
        Err(format!("the threshold must be a finite number, not '{s}'"))
    }
}

impl Comparison {
    fn as_str(self) -> &'static str {
        match self {
            Comparison::Gt => "gt",
            Comparison::Lt => "lt",
            Comparison::Eq => "eq",
        }
    }
}

#[derive(Subcommand, Debug)]
pub enum AlarmCmd {
    /// List.
    #[command(alias = "ls")]
    List(ListArgs),
    /// Show one by name or id.
    #[command(alias = "get")]
    Show(AlarmShow),
    /// Watch a series for a value that crosses.
    Create(AlarmCreate),
    /// Delete by name or id.
    #[command(alias = "rm")]
    Delete(DeleteArgs),
    /// Change what an alarm watches. What is not named keeps its value.
    Update(AlarmUpdate),
}

#[derive(Args, Debug)]
pub struct AlarmShow {
    /// Name or id.
    pub name: String,
    /// Also print the points the deciding series has now.
    #[arg(long)]
    pub samples: bool,
}

#[derive(Args, Debug)]
pub struct AlarmCreate {
    pub name: String,
    #[command(flatten)]
    pub meta: MetaArgs,
    /// Series to watch: a name, or a name and labels such as
    /// `pistack_host_load1{host="h1"}`. With labels the alarm watches every
    /// series that has them; with none, every series of that name.
    #[arg(long)]
    pub metric: String,
    /// Which side of the threshold fires.
    #[arg(long, value_enum)]
    pub comparison: Comparison,
    #[arg(long, value_parser = finite)]
    pub threshold: f64,
    /// How long the value must keep crossing before it fires, in seconds.
    /// The wait is measured on the stamps of the samples themselves.
    #[arg(long = "for-seconds", value_name = "SECONDS", default_value_t = 0)]
    pub for_seconds: u32,
}

#[derive(Args, Debug)]
pub struct AlarmUpdate {
    /// Name or id.
    pub name: String,
    #[arg(long)]
    pub description: Option<String>,
    /// Set a label `key=value` (repeatable).
    #[arg(long = "label", value_name = "K=V")]
    pub labels: Vec<String>,
    /// Series to watch.
    #[arg(long)]
    pub metric: Option<String>,
    #[arg(long, value_enum)]
    pub comparison: Option<Comparison>,
    #[arg(long, value_parser = finite)]
    pub threshold: Option<f64>,
    /// How long the value must keep crossing before it fires, in seconds.
    #[arg(long = "for-seconds", value_name = "SECONDS")]
    pub for_seconds: Option<u32>,
}

#[derive(Subcommand, Debug)]
pub enum MetricCmd {
    /// The series the server holds points for.
    #[command(alias = "ls")]
    List(MetricList),
    /// The last points held for one series.
    #[command(alias = "get")]
    Show(MetricShow),
}

#[derive(Args, Debug)]
pub struct MetricList {
    /// Keep series with this metric name.
    #[arg(long)]
    pub name: Option<String>,
}

#[derive(Args, Debug)]
pub struct MetricShow {
    /// The series, as `pi metric list` prints it: `queue.depth`, or
    /// `pistack_host_load1{host="h1"}`.
    pub series: String,
    /// Keep the last N points.
    #[arg(long, value_name = "N")]
    pub last: Option<usize>,
}

pub async fn alarm(ctx: &Ctx, k: &KindDef, cmd: AlarmCmd, io: &mut Io<'_>) -> CliResult<()> {
    match cmd {
        AlarmCmd::List(a) => {
            let c = ctx.client()?;
            let items = commands::list_all(&c, k, &a).await?;
            print_list(io.stdout, ctx.fmt, k, &items).map_err(Into::into)
        }
        AlarmCmd::Show(a) => show(ctx, a, io).await,
        AlarmCmd::Create(a) => create(ctx, k, a, io).await,
        AlarmCmd::Delete(a) => delete(ctx, k, a, io).await,
        AlarmCmd::Update(a) => update(ctx, k, a, io).await,
    }
}

async fn show(ctx: &Ctx, a: AlarmShow, io: &mut Io<'_>) -> CliResult<()> {
    let c = ctx.client()?;
    let alarm = get_alarm(&c, &a.name).await?;
    if !a.samples {
        return print_one(io.stdout, ctx.fmt, &alarm).map_err(Into::into);
    }
    // With the points behind it, a json or yaml reader gets one document:
    // the record with its points under `samples`.
    match ctx.fmt.output {
        output::Output::Json | output::Output::Yaml => {
            let series = str_at(&alarm, "/status/series");
            let mut doc = alarm.clone();
            let points = if series.is_empty() {
                Vec::new()
            } else {
                samples(&c, Some(&series)).await?
            };
            doc["samples"] = Value::Array(points);
            return print_one(io.stdout, ctx.fmt, &doc).map_err(Into::into);
        }
        _ => print_one(io.stdout, ctx.fmt, &alarm)?,
    }
    let series = str_at(&alarm, "/status/series");
    if series.is_empty() {
        writeln!(
            io.stderr,
            "alarm \"{}\" has no sample yet",
            str_at(&alarm, "/name")
        )?;
        return Ok(());
    }
    let points = samples(&c, Some(&series)).await?;
    if points.is_empty() {
        writeln!(io.stderr, "the server holds no points for {series}")?;
        return Ok(());
    }
    writeln!(io.stdout, "\n{series}")?;
    write!(
        io.stdout,
        "{}",
        output::table(&["TIME", "VALUE"], &point_rows(&points), ctx.fmt.no_headers)
    )?;
    Ok(())
}

/// Read one alarm; a name that is not one is the CLI's own error, the way
/// every other noun puts it.
async fn get_alarm(c: &Client, name_or_id: &str) -> CliResult<Value> {
    c.get_value(&ALARM_KIND.item_path(name_or_id))
        .await
        .map_err(|e| {
            if e.is_not_found() {
                CliError::usage(format!("alarm '{name_or_id}' not found"))
            } else {
                e
            }
        })
}

async fn create(ctx: &Ctx, k: &KindDef, a: AlarmCreate, io: &mut Io<'_>) -> CliResult<()> {
    let spec = json!({
        "name": a.name,
        "metric": a.metric,
        "comparison": a.comparison.as_str(),
        "threshold": a.threshold,
        "for_seconds": a.for_seconds,
    });
    let body = commands::create_body(k, &a.name, &a.meta, spec)?;
    let c = ctx.client()?;
    let v = c.post(k.path, &body).await?;
    commands::report(ctx, io, k, &v, "created")
}

async fn delete(ctx: &Ctx, k: &KindDef, a: DeleteArgs, io: &mut Io<'_>) -> CliResult<()> {
    let c = ctx.client()?;
    let alarm = get_alarm(&c, &a.name).await?;
    let id = str_at(&alarm, "/id");
    if !a.yes && !commands::confirm(io, "delete", &format!("alarm \"{}\"", a.name))? {
        writeln!(io.stderr, "aborted")?;
        return Ok(());
    }
    c.delete(&k.item_path(&id)).await?;
    writeln!(io.stdout, "alarm \"{}\" deleted", a.name)?;
    Ok(())
}

async fn update(ctx: &Ctx, k: &KindDef, a: AlarmUpdate, io: &mut Io<'_>) -> CliResult<()> {
    let touches_spec = a.metric.is_some()
        || a.comparison.is_some()
        || a.threshold.is_some()
        || a.for_seconds.is_some();
    if a.description.is_none() && a.labels.is_empty() && !touches_spec {
        return Err(CliError::usage("nothing to change: name a field to update"));
    }
    let c = ctx.client()?;
    let cur = get_alarm(&c, &a.name).await?;
    let id = str_at(&cur, "/id");
    let mut body = Map::new();
    if let Some(d) = &a.description {
        body.insert("description".into(), json!(d));
    }
    if !a.labels.is_empty() {
        body.insert(
            "labels".into(),
            Value::Object(commands::parse_kv(&a.labels, "--label")?),
        );
    }
    if touches_spec {
        // A partial spec: the server changes what is named and keeps the rest.
        let mut spec = Map::new();
        if let Some(metric) = &a.metric {
            spec.insert("metric".into(), json!(metric));
        }
        if let Some(comparison) = a.comparison {
            spec.insert("comparison".into(), json!(comparison.as_str()));
        }
        if let Some(threshold) = a.threshold {
            spec.insert("threshold".into(), json!(threshold));
        }
        if let Some(for_seconds) = a.for_seconds {
            spec.insert("for_seconds".into(), json!(for_seconds));
        }
        body.insert("spec".into(), Value::Object(spec));
    }
    let v = c.put(&k.item_path(&id), &Value::Object(body)).await?;
    commands::report(ctx, io, k, &v, "updated")
}

pub async fn metric(ctx: &Ctx, cmd: MetricCmd, io: &mut Io<'_>) -> CliResult<()> {
    match cmd {
        MetricCmd::List(a) => list(ctx, a, io).await,
        MetricCmd::Show(a) => show_points(ctx, a, io).await,
    }
}

async fn list(ctx: &Ctx, a: MetricList, io: &mut Io<'_>) -> CliResult<()> {
    let c = ctx.client()?;
    let points = samples(&c, None).await?;
    // Newest point per series, series in key order — the order the window
    // itself keeps them in.
    let mut series: Vec<Value> = Vec::with_capacity(points.len());
    for p in points {
        let key = str_at(&p, "/metric");
        let row = json!({
            "name": series_name(&key),
            "series": key,
            "labels": series_labels(&key),
            "last_value": p.get("value").cloned().unwrap_or(Value::Null),
            "timestamp": p.get("timestamp").cloned().unwrap_or(Value::Null),
        });
        match series.iter().position(|s| str_at(s, "/series") == key) {
            Some(at) => series[at] = row,
            None => series.push(row),
        }
    }
    let series = match a.name {
        Some(name) => series
            .into_iter()
            .filter(|s| str_at(s, "/name") == name)
            .collect(),
        None => series,
    };
    match ctx.fmt.output {
        output::Output::Name => {
            for s in &series {
                writeln!(io.stdout, "{}", str_at(s, "/series"))?;
            }
        }
        _ => print_list(io.stdout, ctx.fmt, &METRIC_KIND, &series)?,
    }
    Ok(())
}

async fn show_points(ctx: &Ctx, a: MetricShow, io: &mut Io<'_>) -> CliResult<()> {
    let c = ctx.client()?;
    let mut points = samples(&c, Some(&a.series)).await?;
    if points.is_empty() {
        return Err(CliError::usage(format!(
            "the server holds no points for series '{}'",
            a.series
        )));
    }
    if let Some(n) = a.last {
        if n < points.len() {
            points.drain(..points.len() - n);
        }
    }
    match ctx.fmt.output {
        output::Output::Json | output::Output::Yaml => {
            print_value(io.stdout, ctx.fmt, &Value::Array(points))?;
        }
        output::Output::Name => {
            for p in &points {
                writeln!(io.stdout, "{}", str_at(p, "/metric"))?;
            }
        }
        output::Output::Table | output::Output::Wide => {
            write!(
                io.stdout,
                "{}",
                output::table(&["TIME", "VALUE"], &point_rows(&points), ctx.fmt.no_headers)
            )?;
        }
    }
    Ok(())
}

/// The window for one series, or the whole window. Gauge answers a bare
/// array here, not a page.
async fn samples(c: &Client, metric: Option<&str>) -> CliResult<Vec<Value>> {
    let path = match metric {
        Some(m) => format!("/gauge/samples?metric={}", urlencode(m)),
        None => "/gauge/samples".to_owned(),
    };
    let v = c.get_value(&path).await?;
    Ok(v.as_array().cloned().unwrap_or_default())
}

fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// `m{a="1"}` -> `m`.
fn series_name(key: &str) -> String {
    gauge::Series::parse(key).name
}

/// `m{a="1",b="x"}` -> `a=1,b=x`.
fn series_labels(key: &str) -> String {
    gauge::Series::parse(key)
        .labels
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join(",")
}

fn metric_name(v: &Value, _: DateTime<Utc>) -> String {
    series_name(&str_at(v, "/series"))
}

fn metric_series(v: &Value, _: DateTime<Utc>) -> String {
    str_at(v, "/series")
}

fn metric_labels(v: &Value, _: DateTime<Utc>) -> String {
    series_labels(&str_at(v, "/series"))
}

fn metric_id(v: &Value, _: DateTime<Utc>) -> String {
    short_id(&metric_series(v, Utc::now()))
}

fn metric_age(v: &Value, now: DateTime<Utc>) -> String {
    output::age(&str_at(v, "/timestamp"), now)
}

fn point_rows(points: &[Value]) -> Vec<Vec<String>> {
    points
        .iter()
        .map(|p| vec![str_at(p, "/timestamp"), str_at(p, "/value")])
        .collect()
}

/// `pi metric` prints from here: there is no resource behind the noun, only
/// the window a server holds in memory.
static METRIC_KIND: KindDef = KindDef {
    noun: "metric",
    aliases: &["metrics"],
    path: "/gauge/samples",
    id_prefix: "metric",
    update: UpdateStyle::None,
    flat_create: false,
    columns: &[
        Column {
            header: "NAME",
            get: metric_name,
        },
        Column {
            header: "SERIES",
            get: metric_series,
        },
        Column {
            header: "LAST-VALUE",
            get: |v, _| output::scalar(v.get("last_value").unwrap_or(&Value::Null)),
        },
        Column {
            header: "AGE",
            get: metric_age,
        },
    ],
    wide: &[
        Column {
            header: "LABELS",
            get: metric_labels,
        },
        Column {
            header: "FULL-SERIES",
            get: metric_id,
        },
    ],
};

static ALARM_KIND: KindDef = KindDef {
    noun: "alarm",
    aliases: &["alarms"],
    path: "/gauge/alarms",
    id_prefix: "alarm",
    update: UpdateStyle::Spec,
    flat_create: false,
    columns: &[
        Column {
            header: "NAME",
            get: |v, _| str_at(v, "/name"),
        },
        Column {
            header: "ID",
            get: |v, _| short_id(&str_at(v, "/id")),
        },
        Column {
            header: "PHASE",
            get: |v, _| str_at(v, "/status/phase"),
        },
        Column {
            header: "METRIC",
            get: |v, _| str_at(v, "/spec/metric"),
        },
        Column {
            header: "COMPARISON",
            get: |v, _| str_at(v, "/spec/comparison"),
        },
        Column {
            header: "THRESHOLD",
            get: |v, _| output::scalar(v.pointer("/spec/threshold").unwrap_or(&Value::Null)),
        },
        Column {
            header: "FOR",
            get: |v, _| output::scalar(v.pointer("/spec/for_seconds").unwrap_or(&Value::Null)),
        },
        Column {
            header: "VALUE",
            get: |v, _| output::scalar(v.pointer("/status/last_value").unwrap_or(&Value::Null)),
        },
        Column {
            header: "AGE",
            get: |v, now| output::age(&str_at(v, "/created_at"), now),
        },
    ],
    wide: &[
        Column {
            header: "SERIES",
            get: |v, _| str_at(v, "/status/series"),
        },
        Column {
            header: "FULL-ID",
            get: |v, _| str_at(v, "/id"),
        },
        Column {
            header: "PROJECT",
            get: |v, _| short_id(&str_at(v, "/project")),
        },
        Column {
            header: "LABELS",
            get: |v, _| {
                v.pointer("/labels")
                    .and_then(Value::as_object)
                    .map(|m| {
                        m.iter()
                            .map(|(k, v)| format!("{k}={}", output::scalar(v)))
                            .collect::<Vec<_>>()
                            .join(",")
                    })
                    .unwrap_or_default()
            },
        },
    ],
};

/// Nouns this file owns, read by `kinds::by_noun`.
pub const KINDS: &[&KindDef] = &[&ALARM_KIND, &METRIC_KIND];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_series_key_is_read_apart() {
        assert_eq!(series_name("queue.depth"), "queue.depth");
        assert_eq!(series_name("m{a=\"1\"}"), "m");
        assert_eq!(series_labels("queue.depth"), "");
        assert_eq!(series_labels("m{a=\"1\"}"), "a=1");
        assert_eq!(series_labels("m{a=\"1\",b=\"x y\"}"), "a=1,b=x y");
    }

    #[test]
    fn a_series_key_survives_the_query() {
        assert_eq!(urlencode("m{host=\"h1\"}"), "m%7Bhost%3D%22h1%22%7D");
    }
}
