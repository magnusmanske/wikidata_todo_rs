//! `stats.php`: statements, references, labels and links of Wikidata items
//! over time, from the JSON files a job made of each dump
//! (`<data_dir>/stats/data/<date>.json`). Tables and stacked bar charts
//! (SVG, made here); `?reverse` stacks the other way round,
//! `action=download` gives all the data as JSON.

use crate::replicas::format_date;
use crate::state::AppState;
use crate::web::{AppError, Params, api_json, render, thousands};
use askama::Template;
use axum::extract::State;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::{Map, Value};
use std::fmt::Write;
use std::path::Path;
use std::sync::Arc;

/// Statement types, as the files name them.
const TYPES: &[&str] = &["itemlink", "string", "globecoordinate", "time", "quantity", "somevalue", "novalue"];
/// Counts per item from this one on are shown together.
const MAX_COUNT: usize = 10;

#[derive(Debug, Deserialize)]
pub struct Request {
    #[serde(default)]
    action: String,
    reverse: Option<String>,
}

#[derive(Template)]
#[template(path = "tools/stats.html")]
struct Page {
    title: &'static str,
    reverse: bool,
    sections: Vec<Section>,
}

struct Section {
    title: String,
    headers: Vec<String>,
    rows: Vec<TableRow>,
    legend: Vec<(String, String)>,
    /// Made here, from numbers and fixed labels.
    chart: String,
}

struct TableRow {
    date: String,
    cells: Vec<(String, Option<String>)>,
}

/// A data series of a chart, one value per date.
struct Series {
    label: String,
    values: Vec<f64>,
}

pub async fn handler(
    State(state): State<Arc<AppState>>,
    Params(request): Params<Request>,
) -> Result<Response, AppError> {
    let snapshots = snapshots(&state.config.data_dir.join("stats/data")).await?;
    if request.action == "download" {
        let all: Map<String, Value> = snapshots.into_iter().collect();
        return Ok(api_json(all, None));
    }
    let reverse = request.reverse.is_some();
    Ok(render(&Page { title: "Statement stats", reverse, sections: sections(&snapshots, reverse) })?.into_response())
}

/// The data files, by date (`20130204`); unreadable ones are left out.
async fn snapshots(dir: &Path) -> anyhow::Result<Vec<(String, Value)>> {
    let mut entries = match tokio::fs::read_dir(dir).await {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(e) => return Err(e.into()),
    };
    let mut snapshots = vec![];
    while let Some(entry) = entries.next_entry().await? {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(date) = name.strip_suffix(".json").filter(|d| !d.is_empty()) else { continue };
        match tokio::fs::read(entry.path()).await.map(|bytes| serde_json::from_slice::<Value>(&bytes)) {
            Ok(Ok(json)) => snapshots.push((date.to_string(), json)),
            Ok(Err(e)) => tracing::debug!("stats: {name}: {e}"),
            Err(e) => tracing::warn!("stats: {name}: {e}"),
        }
    }
    snapshots.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(snapshots)
}

/// A count: a number, or (in some files) a list with the number first.
fn count(value: &Value) -> f64 {
    value.as_f64().or_else(|| value.get(0)?.as_f64()).unwrap_or(0.0)
}

/// Counts per number of things (statements, labels, …): an object keyed by
/// the number, or a list, indexed by it.
fn per_number(value: &Value) -> Vec<(usize, f64)> {
    match value {
        Value::Object(map) => map.iter().filter_map(|(k, v)| Some((k.parse().ok()?, count(v)))).collect(),
        Value::Array(list) => list.iter().enumerate().map(|(k, v)| (k, count(v))).collect(),
        _ => vec![],
    }
}

/// `n` items per number, with everything from [`MAX_COUNT`] on together.
fn buckets(value: &Value) -> [f64; MAX_COUNT + 1] {
    let mut buckets = [0.0; MAX_COUNT + 1];
    for (k, n) in per_number(value) {
        if let Some(bucket) = buckets.get_mut(k.min(MAX_COUNT)) {
            *bucket += n;
        }
    }
    buckets
}

fn sum(value: &Value, keys: &[&str]) -> f64 {
    keys.iter().map(|k| count(&value[*k])).sum()
}

fn number(n: f64) -> String {
    thousands(n.max(0.0).round() as u64)
}

fn percent(n: f64, of: f64) -> Option<String> {
    (of > 0.0).then(|| format!("({:.2}%)", 100.0 * n / of))
}

fn sections(snapshots: &[(String, Value)], reverse: bool) -> Vec<Section> {
    let dates: Vec<String> = snapshots.iter().map(|(date, _)| format_date(date)).collect();
    let mut sections = vec![overview(snapshots, &dates, reverse)];
    for (title, key, types) in [
        ("Referenced statements by statement type", "references_per_statement_type", TYPES),
        ("Statements referenced to Wikipedia by statement type", "wp_reference_statements", TYPES),
        ("Statements referenced to other sources by statement type", "non_wp_reference_statements", TYPES),
        (
            "References by type",
            "references_properties",
            &["wikibase-entityid", "string", "globecoordinate", "time", "unknown"],
        ),
        ("Statement ranks", "rank_per_statement", &["deprecated", "normal", "preferred"]),
    ] {
        sections.push(by_type(snapshots, &dates, reverse, title, key, types));
    }
    for (title, key, unit) in [
        ("Statements per item", "statements_per_item", "statements"),
        ("Labels per item", "labels_per_item", "labels"),
        ("Descriptions per item", "descs_per_item", "descs"),
        ("Wiki(m|p)edia links per item", "links_per_item", "links"),
    ] {
        sections.push(per_item(snapshots, &dates, reverse, title, key, unit));
    }
    sections
}

fn overview(snapshots: &[(String, Value)], dates: &[String], reverse: bool) -> Section {
    let mut series: Vec<Series> =
        ["Unreferenced statements", "Statements referenced to Wikipedia", "Statements referenced to other sources"]
            .iter()
            .map(|label| Series { label: label.to_string(), values: vec![] })
            .collect();
    let mut rows = vec![];
    for ((_, d), date) in snapshots.iter().zip(dates) {
        let total_items = count(&d["total_items"]);
        let referenced_items = count(&d["items_with_referenced_statements"]);
        let statements: f64 = per_number(&d["statements_per_item"]).iter().map(|(k, n)| *k as f64 * n).sum();
        let wp = sum(&d["wp_reference_statements"], TYPES);
        let other = sum(&d["non_wp_reference_statements"], TYPES);
        for (series, value) in series.iter_mut().zip([statements - wp - other, wp, other]) {
            series.values.push(value.max(0.0));
        }
        let cells = vec![
            (number(total_items), None),
            (number(referenced_items), percent(referenced_items, total_items)),
            (number(statements), None),
            (number(wp), percent(wp, statements)),
            (number(other), percent(other, statements)),
        ];
        rows.push(TableRow { date: date.clone(), cells });
    }
    let headers = [
        "Total items",
        "Items with referenced statements",
        "Total statements",
        "Statements referenced to Wikipedia",
        "Statements referenced to other sources",
    ];
    section(
        "Overview",
        headers.iter().map(|h| h.to_string()).collect(),
        rows,
        series,
        dates,
        Palette::Categorical,
        reverse,
    )
}

/// Counts per type, with their total.
fn by_type(
    snapshots: &[(String, Value)],
    dates: &[String],
    reverse: bool,
    title: &str,
    key: &str,
    types: &[&str],
) -> Section {
    let label = |t: &str| if t == "wikibase-entityid" { "itemlink".to_string() } else { t.to_string() };
    let mut series: Vec<Series> = types.iter().map(|t| Series { label: label(t), values: vec![] }).collect();
    let mut rows = vec![];
    for ((_, d), date) in snapshots.iter().zip(dates) {
        let values: Vec<f64> = types.iter().map(|t| count(&d[key][*t])).collect();
        for (series, value) in series.iter_mut().zip(&values) {
            series.values.push(*value);
        }
        let mut cells: Vec<(String, Option<String>)> = values.iter().map(|v| (number(*v), None)).collect();
        cells.push((number(values.iter().sum()), None));
        rows.push(TableRow { date: date.clone(), cells });
    }
    let mut headers: Vec<String> = types.iter().map(|t| label(t)).collect();
    headers.push("Total".to_string());
    section(title, headers, rows, series, dates, Palette::Categorical, reverse)
}

/// Items by how many statements (labels, …) they have.
fn per_item(
    snapshots: &[(String, Value)],
    dates: &[String],
    reverse: bool,
    title: &str,
    key: &str,
    unit: &str,
) -> Section {
    let name = |i: usize| if i == MAX_COUNT { format!("≥{i}") } else { i.to_string() };
    let mut series: Vec<Series> =
        (0..=MAX_COUNT).map(|i| Series { label: format!("{} {unit}", name(i)), values: vec![] }).collect();
    let with_share = key == "statements_per_item";
    let mut rows = vec![];
    for ((_, d), date) in snapshots.iter().zip(dates) {
        let buckets = buckets(&d[key]);
        let total = count(&d["total_items"]);
        for (series, value) in series.iter_mut().zip(buckets) {
            series.values.push(value);
        }
        let cells = buckets.iter().map(|n| (number(*n), percent(*n, total).filter(|_| with_share))).collect();
        rows.push(TableRow { date: date.clone(), cells });
    }
    section(title, (0..=MAX_COUNT).map(name).collect(), rows, series, dates, Palette::Ordinal, reverse)
}

fn section(
    title: &str,
    headers: Vec<String>,
    rows: Vec<TableRow>,
    mut series: Vec<Series>,
    dates: &[String],
    palette: Palette,
    reverse: bool,
) -> Section {
    let colors: Vec<String> = (0..series.len()).map(|i| palette.color(i, series.len())).collect();
    let mut stacked: Vec<(String, Series)> = colors.into_iter().zip(series.drain(..)).collect();
    if reverse {
        stacked.reverse();
    }
    let legend = stacked.iter().map(|(color, s)| (color.clone(), s.label.clone())).collect();
    Section { title: title.to_string(), headers, rows, legend, chart: chart(&stacked, dates) }
}

/// Colours: distinct ones for kinds, a light-to-dark ramp for counts.
#[derive(Clone, Copy)]
enum Palette {
    Categorical,
    Ordinal,
}

/// Categorical colours, in their fixed order (validated for colour vision deficiencies).
const CATEGORICAL: &[&str] = &["#2a78d6", "#eb6834", "#1baf7a", "#eda100", "#e87ba4", "#008300", "#6250d6", "#e34948"];
/// A blue ramp, light to dark, readable on light and dark backgrounds.
const RAMP: &[(u8, u8, u8)] = &[
    (0x86, 0xb6, 0xef),
    (0x6d, 0xa7, 0xec),
    (0x55, 0x98, 0xe7),
    (0x39, 0x87, 0xe5),
    (0x2a, 0x78, 0xd6),
    (0x25, 0x6a, 0xbf),
    (0x1c, 0x5c, 0xab),
    (0x18, 0x4f, 0x95),
];

impl Palette {
    fn color(self, i: usize, n: usize) -> String {
        match self {
            Self::Categorical => CATEGORICAL.get(i % CATEGORICAL.len()).copied().unwrap_or("#888").to_string(),
            Self::Ordinal => {
                // Position on the ramp, interpolated between its steps.
                let at = i as f64 / (n.max(2) - 1) as f64 * (RAMP.len() - 1) as f64;
                let (low, high) = (at.floor() as usize, at.ceil() as usize);
                let (Some(a), Some(b)) = (RAMP.get(low), RAMP.get(high)) else { return "#888".to_string() };
                let t = at - low as f64;
                let mix = |x: u8, y: u8| (x as f64 + (y as f64 - x as f64) * t).round() as u8;
                format!("#{:02x}{:02x}{:02x}", mix(a.0, b.0), mix(a.1, b.1), mix(a.2, b.2))
            }
        }
    }
}

const WIDTH: f64 = 960.0;
const HEIGHT: f64 = 320.0;
/// Room for the value axis on the left, and the years below.
const LEFT: f64 = 48.0;
const BOTTOM: f64 = 22.0;
const TOP: f64 = 8.0;
/// Between stacked segments.
const GAP: f64 = 1.0;

/// A stacked bar chart, one bar per date, the first series at the bottom.
fn chart(series: &[(String, Series)], dates: &[String]) -> String {
    let n = dates.len();
    let totals: Vec<f64> =
        (0..n).map(|j| series.iter().map(|(_, s)| s.values.get(j).copied().unwrap_or(0.0)).sum()).collect();
    let max = totals.iter().copied().fold(0.0, f64::max);
    let (step, top) = ticks(max);
    let plot_h = HEIGHT - BOTTOM - TOP;
    let slot = (WIDTH - LEFT) / n.max(1) as f64;
    let bar = (slot * 0.8).min(24.0);
    let y = |v: f64| TOP + plot_h - v / top * plot_h;
    let mut svg = format!(
        r#"<svg class="chart" viewBox="0 0 {WIDTH} {HEIGHT}" role="img" aria-label="Stacked bars, one per date">"#
    );
    let mut v = 0.0;
    while v <= top + step / 2.0 {
        let _ = write!(svg, r#"<line class="grid" x1="{LEFT}" x2="{WIDTH}" y1="{0:.1}" y2="{0:.1}"/>"#, y(v));
        let _ = write!(
            svg,
            r#"<text class="axis" x="{}" y="{:.1}" text-anchor="end">{}</text>"#,
            LEFT - 6.0,
            y(v) + 4.0,
            compact(v)
        );
        v += step;
    }
    let mut base = vec![0.0; n];
    for (color, s) in series {
        let mut path = String::new();
        for (j, value) in s.values.iter().enumerate().take(n) {
            let height = value / top * plot_h;
            if let Some(b) = base.get_mut(j) {
                if height > GAP {
                    let x = LEFT + j as f64 * slot + (slot - bar) / 2.0;
                    let _ = write!(path, "M{x:.1},{:.1}h{bar:.1}v{:.1}h-{bar:.1}z", y(*b + value), height - GAP);
                }
                *b += value;
            }
        }
        let _ = write!(svg, r#"<path fill="{color}" d="{path}"/>"#);
    }
    let mut year = "";
    for (j, date) in dates.iter().enumerate() {
        let x = LEFT + j as f64 * slot;
        let this_year = date.get(0..4).unwrap_or_default();
        if this_year != year {
            year = this_year;
            let _ = write!(svg, r#"<text class="axis" x="{x:.1}" y="{}">{year}</text>"#, HEIGHT - 6.0);
        }
        let mut tip = escape(date);
        for (_, s) in series.iter().rev() {
            let _ = write!(tip, "\n{}: {}", escape(&s.label), number(s.values.get(j).copied().unwrap_or(0.0)));
        }
        let _ = write!(
            svg,
            r#"<rect class="hit" x="{x:.1}" y="{TOP}" width="{slot:.1}" height="{plot_h}"><title>{tip}</title></rect>"#
        );
    }
    svg + "</svg>"
}

/// A round step for about four gridlines up to `max`, and the axis top.
fn ticks(max: f64) -> (f64, f64) {
    if max <= 0.0 {
        return (1.0, 1.0);
    }
    let raw = max / 4.0;
    let magnitude = 10f64.powf(raw.log10().floor());
    let step = [1.0, 2.0, 5.0, 10.0].iter().map(|f| f * magnitude).find(|s| *s >= raw).unwrap_or(magnitude * 10.0);
    (step, (max / step).ceil() * step)
}

/// `1500000` as `1.5M`.
fn compact(v: f64) -> String {
    let (scaled, suffix) = match v {
        v if v >= 1e9 => (v / 1e9, "B"),
        v if v >= 1e6 => (v / 1e6, "M"),
        v if v >= 1e3 => (v / 1e3, "K"),
        v => (v, ""),
    };
    let text = format!("{scaled:.1}");
    format!("{}{suffix}", text.strip_suffix(".0").unwrap_or(&text))
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::test_support::get;
    use serde_json::json;

    #[test]
    fn counts_in_either_shape() {
        assert_eq!(buckets(&json!({"0": 5, "2": 1, "11": 2, "250": 3}))[..3], [5.0, 0.0, 1.0]);
        assert_eq!(buckets(&json!({"11": 2, "250": 3}))[MAX_COUNT], 5.0);
        assert_eq!(buckets(&json!([7, 1]))[..2], [7.0, 1.0]);
        assert_eq!(count(&json!([3])), 3.0);
        assert_eq!(count(&json!(null)), 0.0);
    }

    #[test]
    fn axis() {
        assert_eq!(ticks(95_000_000.0), (50_000_000.0, 100_000_000.0));
        assert_eq!(ticks(0.0), (1.0, 1.0));
        assert_eq!(compact(1_500_000.0), "1.5M");
        assert_eq!(compact(20_000_000.0), "20M");
        assert_eq!(compact(0.0), "0");
        assert_eq!(Palette::Ordinal.color(0, 11), "#86b6ef");
        assert_eq!(Palette::Ordinal.color(10, 11), "#184f95");
    }

    #[tokio::test]
    async fn page_and_download() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("stats/data");
        std::fs::create_dir_all(&data).unwrap();
        let snapshot = json!({
            "statements_per_item": {"0": 2, "1": 3, "12": 1}, "total_items": 6, "items_with_referenced_statements": 2,
            "wp_reference_statements": {"string": 1}, "non_wp_reference_statements": {"itemlink": 4},
            "labels_per_item": [6], "rank_per_statement": {"normal": 15},
        });
        std::fs::write(data.join("20211027.json"), snapshot.to_string()).unwrap();
        std::fs::write(data.join("20211110.json"), "").unwrap();
        let page = get(dir.path(), "/stats/?reverse").await;
        assert_eq!(page.status.as_u16(), 200);
        assert!(page.body.contains("<th>2021-10-27</th><td class=\"num\">6</td>"), "{}", page.body);
        assert!(page.body.contains("4 <small class=\"muted\">(26.67%)</small>"));
        assert!(page.body.contains("<svg class=\"chart\""));
        let download = get(dir.path(), "/stats/?action=download").await;
        assert_eq!(serde_json::from_str::<Value>(&download.body).unwrap()["20211027"]["total_items"], 6);
    }
}
