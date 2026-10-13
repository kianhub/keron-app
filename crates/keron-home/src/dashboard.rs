//! The `dashboard` kind: what the Mac mini processed (model tokens and their
//! estimated dollars, job runs, polls, door traffic, Clef), as the door's
//! `GET /memory/dashboard` serves it to the Mini card and its page on Home.
//!
//! The mini does the counting and the formatting: every figure arrives as
//! text (`"1.23M"`, `"$4.10"`) beside the raw `value` that sizes its bar,
//! cell or share. What the app works out is layout, and it does that here,
//! once per payload, so drawing a frame does no date math: the heatmap's
//! grid of 53 week columns, Monday first, ending at the mini's today, with
//! each day's level and the month labels over it. The one number the app
//! writes itself is a chart's scale, which the server doesn't send
//! ([`scale`], [`scale_text`]).
//!
//! Lenient like every kind: unknown fields are ignored, a missing period,
//! chart or row is skipped, a value that isn't a number is 0, and a series
//! is padded or cut to its chart's labels. Parts that are there but can't be
//! read (not an object, or without their main text) are skipped and counted
//! into the payload's `errors` as one line.

use std::collections::HashMap;

use chrono::{Datelike, Duration, NaiveDate};
use serde_json::Value;

use crate::kinds::{Object, field, number, parse_date, text};

/// The heatmap's width in weeks: a year, plus the week it started in.
pub const WEEKS: usize = 53;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Period {
    Day,
    Week,
    Month,
    Year,
}

impl Period {
    pub const ALL: [Period; 4] = [Period::Day, Period::Week, Period::Month, Period::Year];

    /// The key in `periods` and `card.period`.
    pub fn name(self) -> &'static str {
        match self {
            Period::Day => "day",
            Period::Week => "week",
            Period::Month => "month",
            Period::Year => "year",
        }
    }

    /// The period switch's word for it.
    pub fn title(self) -> &'static str {
        match self {
            Period::Day => "Day",
            Period::Week => "Week",
            Period::Month => "Month",
            Period::Year => "Year",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        Period::ALL
            .into_iter()
            .find(|period| period.name() == text.trim())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Dashboard {
    /// A footnote, like how the dollars are estimated.
    pub note: Option<String>,
    pub card: Card,
    /// The periods sent, in [`Period::ALL`]'s order.
    pub periods: Vec<(Period, PeriodData)>,
    pub heatmap: Option<Heatmap>,
}

/// What the card on Home shows: some tiles and one chart of one period.
#[derive(Clone, Debug, PartialEq)]
pub struct Card {
    pub period: Period,
    /// Tile ids, in the card's order.
    pub tiles: Vec<String>,
    pub chart: Option<String>,
}

/// One period's figures.
#[derive(Clone, Debug, PartialEq)]
pub struct PeriodData {
    /// "Today", "7 days".
    pub label: String,
    /// "6–12 Oct".
    pub range: Option<String>,
    pub tiles: Vec<Tile>,
    pub charts: Vec<Chart>,
    pub breakdowns: Vec<Breakdown>,
}

/// One big figure with its label.
#[derive(Clone, Debug, PartialEq)]
pub struct Tile {
    pub id: String,
    pub label: String,
    /// The figure as shown ("1.23M").
    pub text: String,
    /// A quieter line under it ("≈ $4.10 est.").
    pub sub: Option<String>,
    /// The change from the period before ("+12%").
    pub delta: Option<String>,
    pub tone: Tone,
}

/// Which way a delta went. Up isn't bad or good here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Up,
    Down,
    Flat,
}

/// Stacked bars, one per bucket (an hour, a day, a month).
#[derive(Clone, Debug, PartialEq)]
pub struct Chart {
    pub id: String,
    pub title: String,
    /// "tokens", "usd", "neurons".
    pub unit: Option<String>,
    pub labels: Vec<String>,
    /// Bottom first, each exactly as long as `labels`.
    pub series: Vec<Series>,
    /// One tooltip per bucket, as long as `labels` ("" where none came).
    pub texts: Vec<String>,
    /// A horizontal threshold, like Clef's free neurons a day.
    pub line: Option<Line>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Series {
    pub id: String,
    pub label: String,
    pub values: Vec<f64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub value: f64,
    pub label: Option<String>,
}

impl Chart {
    /// Each bucket's stack.
    pub fn totals(&self) -> Vec<f64> {
        (0..self.labels.len())
            .map(|ix| self.series.iter().map(|series| series.values[ix]).sum())
            .collect()
    }

    /// The top of the chart: its tallest stack or its line, rounded up by
    /// [`scale`]. 0 when the chart has nothing yet.
    pub fn scale(&self) -> f64 {
        let tallest = self.totals().into_iter().fold(0.0, f64::max);
        scale(tallest.max(self.line.as_ref().map_or(0.0, |line| line.value)))
    }
}

/// A table: rows with cells under the columns, and a share bar each.
#[derive(Clone, Debug, PartialEq)]
pub struct Breakdown {
    pub id: String,
    pub title: String,
    pub columns: Vec<String>,
    pub rows: Vec<Row>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub label: String,
    /// Under `columns`, as shown.
    pub cells: Vec<String>,
    /// Of the table's first column, 0 to 1.
    pub share: f32,
    /// A quieter line under the label ("Sonnet 5.5 · hourly").
    pub sub: Option<String>,
}

/// A GitHub-style year of days.
#[derive(Clone, Debug, PartialEq)]
pub struct Heatmap {
    /// "Tokens per day".
    pub label: String,
    /// Every day from the Monday of the first week through the mini's
    /// today; day `ix` is in week column `ix / 7`, row `ix % 7` (Monday is
    /// row 0).
    pub days: Vec<Day>,
    /// Week columns that start a month, with the month's short name.
    pub months: Vec<(usize, String)>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Day {
    pub date: NaiveDate,
    pub value: f64,
    /// 0 (nothing) to 4.
    pub level: u8,
    /// The tooltip, as served; `None` for a day the server didn't send.
    pub text: Option<String>,
}

impl Heatmap {
    /// The day in week `column`, weekday `row` (Monday 0), if it isn't past
    /// today.
    pub fn day(&self, column: usize, row: usize) -> Option<&Day> {
        (column < WEEKS && row < 7)
            .then(|| self.days.get(column * 7 + row))
            .flatten()
    }

    /// Days with anything at all.
    pub fn active_days(&self) -> usize {
        self.days.iter().filter(|day| day.value > 0.0).count()
    }

    /// The day with the most, the latest of equals.
    pub fn busiest(&self) -> Option<&Day> {
        self.days
            .iter()
            .filter(|day| day.value > 0.0)
            .max_by(|a, b| a.value.total_cmp(&b.value))
    }
}

impl Dashboard {
    pub fn is_empty(&self) -> bool {
        self.periods.is_empty() && self.heatmap.is_none()
    }

    pub fn period(&self, period: Period) -> Option<&PeriodData> {
        self.periods
            .iter()
            .find(|(p, _)| *p == period)
            .map(|(_, data)| data)
    }

    /// `period` if it was sent, else the first one that was.
    pub fn period_or_first(&self, period: Period) -> Option<(Period, &PeriodData)> {
        self.period(period)
            .map(|data| (period, data))
            .or_else(|| self.periods.first().map(|(p, data)| (*p, data)))
    }

    /// The card's tiles: those it names, in its order, or the period's first
    /// four when it names none.
    pub fn card_tiles(&self) -> Vec<&Tile> {
        let Some((_, data)) = self.period_or_first(self.card.period) else {
            return Vec::new();
        };
        if self.card.tiles.is_empty() {
            return data.tiles.iter().take(4).collect();
        }
        self.card
            .tiles
            .iter()
            .filter_map(|id| data.tiles.iter().find(|tile| tile.id == *id))
            .collect()
    }

    /// The card's chart: the one it names, or the period's first.
    pub fn card_chart(&self) -> Option<&Chart> {
        let (_, data) = self.period_or_first(self.card.period)?;
        match &self.card.chart {
            Some(id) => data.charts.iter().find(|chart| chart.id == *id),
            None => data.charts.first(),
        }
    }
}

/// Parse a dashboard payload's body. Unreadable parts are counted into
/// `errors`.
pub(crate) fn parse(o: &Object, errors: &mut Vec<String>) -> Dashboard {
    let mut skipped = 0;
    let periods = match o.get("periods") {
        Some(Value::Object(periods)) => Period::ALL
            .into_iter()
            .filter_map(|period| {
                let data = periods.get(period.name())?;
                let parsed = data
                    .as_object()
                    .map(|data| period_data(period, data, &mut skipped));
                skipped += usize::from(parsed.is_none());
                parsed.map(|data| (period, data))
            })
            .collect(),
        _ => Vec::new(),
    };
    let card = o.get("card").and_then(Value::as_object);
    let card = Card {
        period: card
            .and_then(|card| field(card, "period"))
            .and_then(|name| Period::parse(&name))
            .unwrap_or(Period::Day),
        tiles: match card.and_then(|card| card.get("tiles")) {
            Some(Value::Array(ids)) => ids.iter().filter_map(text).collect(),
            _ => Vec::new(),
        },
        chart: card.and_then(|card| field(card, "chart")),
    };
    let heatmap = o.get("heatmap").and_then(Value::as_object).and_then(|map| {
        let end = field(map, "end")
            .or_else(|| field(o, "today"))
            .and_then(|end| parse_date(&end))?;
        Some(heatmap(map, end, &mut skipped))
    });
    if skipped > 0 {
        let parts = if skipped == 1 { "part" } else { "parts" };
        errors.push(format!(
            "{skipped} dashboard {parts} didn't parse and were skipped"
        ));
    }
    Dashboard {
        note: field(o, "note"),
        card,
        periods,
        heatmap,
    }
}

/// The objects in `o[key]` that `parse` reads, counting the rest.
fn each<T>(
    o: &Object,
    key: &str,
    skipped: &mut usize,
    mut parse: impl FnMut(&Object, &mut usize) -> Option<T>,
) -> Vec<T> {
    let Some(Value::Array(items)) = o.get(key) else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| {
            let parsed = item.as_object().and_then(|item| parse(item, skipped));
            *skipped += usize::from(parsed.is_none());
            parsed
        })
        .collect()
}

/// A count or an amount: 0 when it's missing, not a number, or below 0.
fn amount(o: &Object, key: &str) -> f64 {
    o.get(key).and_then(number).unwrap_or(0.0).max(0.0)
}

fn period_data(period: Period, o: &Object, skipped: &mut usize) -> PeriodData {
    PeriodData {
        label: field(o, "label").unwrap_or_else(|| period.title().to_string()),
        range: field(o, "range"),
        tiles: each(o, "tiles", skipped, |o, _| tile(o)),
        charts: each(o, "charts", skipped, chart),
        breakdowns: each(o, "breakdowns", skipped, breakdown),
    }
}

fn tile(o: &Object) -> Option<Tile> {
    let id = field(o, "id").unwrap_or_default();
    // A tile sent without its text shows its value as written.
    let text = field(o, "text").or_else(|| field(o, "value"))?;
    Some(Tile {
        label: field(o, "label").unwrap_or_else(|| id.clone()),
        id,
        text,
        sub: field(o, "sub"),
        delta: field(o, "delta"),
        tone: match field(o, "tone").as_deref() {
            Some("up") => Tone::Up,
            Some("down") => Tone::Down,
            _ => Tone::Flat,
        },
    })
}

fn chart(o: &Object, skipped: &mut usize) -> Option<Chart> {
    let Some(Value::Array(labels)) = o.get("labels") else {
        return None;
    };
    let labels: Vec<String> = labels
        .iter()
        .map(|label| text(label).unwrap_or_default())
        .collect();
    if labels.is_empty() {
        return None;
    }
    let len = labels.len();
    let series = each(o, "series", skipped, |s, _| {
        let id = field(s, "id").or_else(|| field(s, "label"))?;
        let mut values: Vec<f64> = match s.get("values") {
            Some(Value::Array(values)) => values
                .iter()
                .map(|v| number(v).unwrap_or(0.0).max(0.0))
                .collect(),
            _ => Vec::new(),
        };
        values.resize(len, 0.0);
        Some(Series {
            label: field(s, "label").unwrap_or_else(|| id.clone()),
            id,
            values,
        })
    });
    let mut texts: Vec<String> = match o.get("texts") {
        Some(Value::Array(texts)) => texts.iter().map(|t| text(t).unwrap_or_default()).collect(),
        _ => Vec::new(),
    };
    texts.resize(len, String::new());
    let id = field(o, "id").unwrap_or_default();
    Some(Chart {
        title: field(o, "title").unwrap_or_else(|| id.clone()),
        id,
        unit: field(o, "unit"),
        labels,
        series,
        texts,
        line: o.get("line").and_then(Value::as_object).and_then(|line| {
            Some(Line {
                value: line.get("value").and_then(number)?.max(0.0),
                label: field(line, "label"),
            })
        }),
    })
}

fn breakdown(o: &Object, skipped: &mut usize) -> Option<Breakdown> {
    let id = field(o, "id").unwrap_or_default();
    Some(Breakdown {
        title: field(o, "title").unwrap_or_else(|| id.clone()),
        id,
        columns: match o.get("columns") {
            Some(Value::Array(columns)) => columns
                .iter()
                .map(|c| text(c).unwrap_or_default())
                .collect(),
            _ => Vec::new(),
        },
        rows: each(o, "rows", skipped, |r, _| {
            Some(Row {
                label: field(r, "label").or_else(|| field(r, "id"))?,
                cells: match r.get("cells") {
                    Some(Value::Array(cells)) => {
                        cells.iter().map(|c| text(c).unwrap_or_default()).collect()
                    }
                    _ => Vec::new(),
                },
                share: r
                    .get("share")
                    .and_then(number)
                    .map_or(0.0, |share| share.clamp(0.0, 1.0) as f32),
                sub: field(r, "sub"),
            })
        }),
    })
}

fn heatmap(o: &Object, end: NaiveDate, skipped: &mut usize) -> Heatmap {
    let start = end
        - Duration::days(i64::from(end.weekday().num_days_from_monday()))
        - Duration::weeks(WEEKS as i64 - 1);
    // A day sent twice counts as its last.
    let mut sent: HashMap<NaiveDate, (f64, Option<String>)> = HashMap::new();
    for (date, value, text) in each(o, "days", skipped, |d, _| {
        let date = field(d, "date").and_then(|date| parse_date(&date))?;
        Some((date, amount(d, "value"), field(d, "text")))
    }) {
        sent.insert(date, (value, text));
    }
    let dates: Vec<NaiveDate> = start.iter_days().take_while(|date| *date <= end).collect();
    let levels = thresholds(o.get("levels")).unwrap_or_else(|| {
        let mut values: Vec<f64> = dates
            .iter()
            .filter_map(|date| sent.get(date).map(|(value, _)| *value))
            .filter(|value| *value > 0.0)
            .collect();
        quartiles(&mut values)
    });
    let days = dates
        .into_iter()
        .map(|date| {
            let (value, text) = sent.remove(&date).unwrap_or((0.0, None));
            Day {
                date,
                value,
                level: level(value, &levels),
                text,
            }
        })
        .collect();
    // A column's month label goes over the first Monday of that month.
    let months = (0..WEEKS)
        .filter_map(|column| {
            let monday = start + Duration::weeks(column as i64);
            (monday.day() <= 7).then(|| (column, monday.format("%b").to_string()))
        })
        .collect();
    Heatmap {
        label: field(o, "label").unwrap_or_else(|| "Every day".to_string()),
        days,
        months,
    }
}

/// Three numbers, in ascending order; `None` unless there are three.
fn thresholds(levels: Option<&Value>) -> Option<[f64; 3]> {
    let Some(Value::Array(levels)) = levels else {
        return None;
    };
    let mut levels: Vec<f64> = levels.iter().filter_map(number).collect();
    levels.sort_by(f64::total_cmp);
    levels.try_into().ok()
}

/// Thresholds at a quarter, half and three quarters of the way through the
/// non-zero days, for a heatmap sent without any.
fn quartiles(values: &mut [f64]) -> [f64; 3] {
    values.sort_by(f64::total_cmp);
    let at = |share: f64| {
        let ix = ((values.len().saturating_sub(1)) as f64 * share).round() as usize;
        values.get(ix).copied().unwrap_or(f64::INFINITY)
    };
    [at(0.25), at(0.5), at(0.75)]
}

/// 0 for nothing, 1 for anything, then 2, 3 and 4 from each threshold up.
pub fn level(value: f64, levels: &[f64; 3]) -> u8 {
    if value <= 0.0 {
        return 0;
    }
    1 + levels
        .iter()
        .filter(|threshold| value >= **threshold)
        .count() as u8
}

/// `value` rounded up to 1, 2 or 5 times a power of ten, the top of a chart
/// whose tallest bar is `value`; 0 stays 0.
pub fn scale(value: f64) -> f64 {
    if !value.is_finite() || value <= 0.0 {
        return 0.0;
    }
    let power = 10f64.powi(value.log10().floor() as i32);
    [1.0, 2.0, 5.0, 10.0]
        .into_iter()
        .map(|step| step * power)
        .find(|top| value <= top * (1.0 + 1e-9))
        .unwrap_or(10.0 * power)
}

/// A [`scale`] written the way the server writes figures: `500`, `2k`,
/// `1M`, and in dollars `$5` or `$0.50`.
pub fn scale_text(value: f64, unit: Option<&str>) -> String {
    let trimmed = |n: f64| {
        let text = format!("{n:.2}");
        text.trim_end_matches('0').trim_end_matches('.').to_string()
    };
    if unit == Some("usd") {
        if value <= 0.0 {
            return "$0".to_string();
        }
        if value >= 1.0 {
            return format!("${}", trimmed(value));
        }
        let decimals = ((-value.log10()).ceil() as usize).max(2);
        return format!("${value:.decimals$}");
    }
    for (size, suffix) in [(1e9, "B"), (1e6, "M"), (1e3, "k")] {
        if value >= size {
            return format!("{}{suffix}", trimmed(value / size));
        }
    }
    trimmed(value)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::{Body, Kind, kinds::parse_payload};

    fn parsed(served: Value) -> (Dashboard, Vec<String>) {
        let payload = parse_payload(Kind::Dashboard, &served).unwrap();
        let Body::Dashboard(dashboard) = payload.body else {
            panic!("not a dashboard")
        };
        (*dashboard, payload.errors)
    }

    fn date(text: &str) -> NaiveDate {
        parse_date(text).unwrap()
    }

    /// The contract's example payload, one hour wide.
    fn served() -> Value {
        json!({
            "source": "dashboard", "kind": "dashboard",
            "updated": "2026-10-12T18:40:00-05:00", "errors": [],
            "today": "2026-10-12", "since": "2026-10-07",
            "note": "Dollars are API-equivalent at list price.",
            "card": {"period": "day", "tiles": ["tokens", "cost", "runs", "errors"], "chart": "tokens"},
            "periods": {
                "day": {
                    "label": "Today", "range": "Mon 12 Oct",
                    "tiles": [
                        {"id": "tokens", "label": "Tokens", "value": 1234567, "text": "1.23M",
                         "sub": "≈ $4.10 est.", "delta": "+12%", "tone": "up"},
                        {"id": "cost", "label": "$ est.", "value": 4.1, "text": "$4.10",
                         "sub": "API-equivalent", "delta": "-3%", "tone": "down"},
                        {"id": "runs", "label": "Runs", "value": 31, "text": "31"},
                        {"id": "model-calls", "label": "Model calls", "value": 40, "text": "40"},
                        {"id": "errors", "label": "Errors", "value": 1, "text": "1", "tone": "flat"}
                    ],
                    "charts": [
                        {"id": "tokens", "title": "Tokens by job", "unit": "tokens",
                         "labels": ["00", "01", "02"],
                         "series": [
                             {"id": "loose-ends", "label": "Loose ends", "values": [0, 1200, 300]},
                             {"id": "naps", "label": "Naps", "values": [500, 0, 0]}
                         ],
                         "texts": ["Mon 12 Oct 00:00 · 500 tokens", "Mon 12 Oct 01:00 · 1.2k tokens · Loose ends 1.2k", "Mon 12 Oct 02:00 · 300 tokens"]},
                        {"id": "clef", "title": "Clef neurons", "unit": "neurons",
                         "labels": ["00", "01", "02"],
                         "series": [{"id": "neurons", "label": "Neurons", "values": [10, 20, 30]}],
                         "texts": ["a", "b", "c"],
                         "line": {"value": 10000, "label": "free a day"}}
                    ],
                    "breakdowns": [
                        {"id": "jobs", "title": "By job", "columns": ["Tokens", "$ est.", "Runs", "Errors"],
                         "rows": [{"id": "loose-ends", "label": "Loose ends", "value": 412000,
                                   "cells": ["412k", "$1.20", "24", "1"], "share": 0.41, "sub": "Sonnet 5.5 · hourly"}]}
                    ]
                },
                "week": {"label": "7 days", "range": "6–12 Oct", "tiles": [], "charts": [], "breakdowns": []}
            },
            "heatmap": {
                "label": "Tokens per day", "unit": "tokens", "end": "2026-10-12", "week_start": "monday",
                "days": [{"date": "2026-10-11", "value": 1234567, "text": "Sun 11 Oct · 1.2M tokens · ≈ $4.10 · 31 runs"}],
                "levels": [100000, 500000, 1000000]
            }
        })
    }

    #[test]
    fn the_contracts_payload_parses_as_served() {
        let (dashboard, errors) = parsed(served());
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(
            dashboard.note.as_deref(),
            Some("Dollars are API-equivalent at list price.")
        );
        let periods: Vec<Period> = dashboard.periods.iter().map(|(p, _)| *p).collect();
        assert_eq!(periods, [Period::Day, Period::Week]);

        let tiles: Vec<(&str, &str, Tone)> = dashboard
            .card_tiles()
            .iter()
            .map(|tile| (tile.id.as_str(), tile.text.as_str(), tile.tone))
            .collect();
        assert_eq!(
            tiles,
            [
                ("tokens", "1.23M", Tone::Up),
                ("cost", "$4.10", Tone::Down),
                ("runs", "31", Tone::Flat),
                ("errors", "1", Tone::Flat),
            ]
        );
        let chart = dashboard.card_chart().unwrap();
        assert_eq!(chart.title, "Tokens by job");
        assert_eq!(chart.totals(), [500.0, 1200.0, 300.0]);
        assert_eq!(chart.scale(), 2000.0);
        assert_eq!(chart.series[0].label, "Loose ends");

        let day = dashboard.period(Period::Day).unwrap();
        let clef = &day.charts[1];
        assert_eq!(
            clef.line,
            Some(Line {
                value: 10000.0,
                label: Some("free a day".to_string())
            })
        );
        // The threshold sets the scale when the bars are under it.
        assert_eq!(clef.scale(), 10000.0);
        let row = &day.breakdowns[0].rows[0];
        assert_eq!(row.cells, ["412k", "$1.20", "24", "1"]);
        assert_eq!(row.share, 0.41);
        assert_eq!(row.sub.as_deref(), Some("Sonnet 5.5 · hourly"));
        assert_eq!(
            dashboard.period(Period::Week).unwrap().range.as_deref(),
            Some("6–12 Oct")
        );

        let heatmap = dashboard.heatmap.unwrap();
        let sunday = heatmap.day(51, 6).unwrap();
        assert_eq!(sunday.date, date("2026-10-11"));
        assert_eq!((sunday.value, sunday.level), (1234567.0, 4));
        assert_eq!(heatmap.busiest().map(|day| day.date), Some(sunday.date));
        assert_eq!(heatmap.active_days(), 1);
    }

    #[test]
    fn a_payload_with_bad_parts_still_draws_the_rest() {
        let (dashboard, errors) = parsed(json!({
            "card": {"period": "month", "chart": "missing"},
            "periods": {
                "day": {
                    "tiles": [
                        {"id": "tokens", "value": "12", "extra": true},
                        {"id": "no-text"},
                        "not a tile"
                    ],
                    "charts": [
                        {"id": "runs", "labels": ["a", "b", "c"],
                         "series": [
                             {"id": "naps", "values": [1, "2", null, 4, 5]},
                             {"id": "judge", "values": [-3]},
                             {"values": [1, 1, 1]}
                         ],
                         "texts": ["only one"]},
                        {"id": "empty", "labels": []}
                    ],
                    "breakdowns": [{"rows": [{"label": "A", "share": 7}, {"cells": []}]}]
                },
                "week": "not a period",
                "decade": {"label": "Ten years"}
            },
            "heatmap": {"days": [{"date": "2026-10-12", "value": "nope"}, {"date": "12 Oct"}]},
            "today": "2026-10-12"
        }));
        // The tile without text, the string, the period that isn't an
        // object, the series without an id, the chart without labels, the
        // row without a label and the day with a bad date.
        assert_eq!(errors, ["7 dashboard parts didn't parse and were skipped"]);
        let periods: Vec<Period> = dashboard.periods.iter().map(|(p, _)| *p).collect();
        assert_eq!(periods, [Period::Day]);
        let day = dashboard.period(Period::Day).unwrap();
        assert_eq!(day.label, "Day");
        // A tile without its text shows its value as written.
        let tile = &day.tiles[0];
        assert_eq!((tile.label.as_str(), tile.text.as_str()), ("tokens", "12"));

        // Padded or cut to the labels; anything not a number, or below 0, is 0.
        let chart = &day.charts[0];
        assert_eq!(chart.series[0].values, [1.0, 2.0, 0.0]);
        assert_eq!(chart.series[1].values, [0.0, 0.0, 0.0]);
        assert_eq!(chart.texts, ["only one", "", ""]);
        assert_eq!(day.breakdowns[0].rows[0].share, 1.0);

        // A month the server didn't send: the card shows the first period,
        // its first four tiles, and no chart for a name that isn't there.
        assert_eq!(
            dashboard.period_or_first(Period::Month).map(|(p, _)| p),
            Some(Period::Day)
        );
        assert_eq!(dashboard.card_tiles().len(), 1);
        assert_eq!(dashboard.card_chart(), None);

        // The heatmap ends at the payload's today when it names no end.
        let last = dashboard.heatmap.unwrap().days.pop().unwrap();
        assert_eq!((last.date, last.value), (date("2026-10-12"), 0.0));

        let (empty, errors) = parsed(json!({"periods": [], "heatmap": {"days": []}}));
        assert!(empty.is_empty());
        assert!(errors.is_empty());
    }

    #[test]
    fn the_heatmap_is_53_weeks_monday_first_ending_at_the_minis_today() {
        let grid = |end: &str| {
            parsed(json!({"heatmap": {"end": end, "days": []}}))
                .0
                .heatmap
                .unwrap()
        };
        // Ending on a Monday: the last column has just that day.
        let monday = grid("2026-10-12");
        assert_eq!(monday.days.len(), 52 * 7 + 1);
        assert_eq!(monday.days[0].date, date("2025-10-13"));
        assert_eq!(monday.days[0].date.weekday(), chrono::Weekday::Mon);
        assert_eq!(
            monday.day(52, 0).map(|day| day.date),
            Some(date("2026-10-12"))
        );
        assert_eq!(monday.day(52, 1), None);
        assert_eq!(
            monday.day(0, 6).map(|day| day.date),
            Some(date("2025-10-19"))
        );
        // Ending on a Sunday: every column is full.
        let sunday = grid("2026-10-18");
        assert_eq!(sunday.days.len(), 53 * 7);
        assert_eq!(
            sunday.day(52, 6).map(|day| day.date),
            Some(date("2026-10-18"))
        );
        assert_eq!(sunday.day(53, 0), None);
    }

    #[test]
    fn month_labels_sit_over_each_months_first_monday() {
        let heatmap = parsed(json!({"heatmap": {"end": "2026-10-12", "days": []}}))
            .0
            .heatmap
            .unwrap();
        let months: Vec<(usize, &str)> = heatmap
            .months
            .iter()
            .map(|(column, name)| (*column, name.as_str()))
            .collect();
        // The grid starts mid-October 2025, so its first label is November's.
        assert_eq!(months.len(), 12);
        assert_eq!(months[0], (3, "Nov"));
        assert_eq!(months[1], (7, "Dec"));
        assert_eq!(months[11], (51, "Oct"));
        for (column, _) in &heatmap.months {
            let monday = heatmap.day(*column, 0).unwrap().date;
            assert!(monday.day() <= 7, "{monday}");
        }
    }

    #[test]
    fn levels_follow_the_servers_thresholds_or_the_days_quartiles() {
        let levels = [100.0, 500.0, 1000.0];
        let at = |value: f64| level(value, &levels);
        assert_eq!(
            [0.0, 1.0, 99.0, 100.0, 499.0, 500.0, 999.0, 1000.0, 1e9].map(at),
            [0, 1, 1, 2, 2, 3, 3, 4, 4]
        );

        // Out of order is sorted; fewer than three falls back to quartiles
        // of the days that had anything.
        let days = |levels: Value| {
            let days: Vec<Value> = (1..=8)
                .map(|n| json!({"date": format!("2026-10-0{n}"), "value": n * 10}))
                .collect();
            parsed(json!({"heatmap": {"end": "2026-10-08", "days": days, "levels": levels}}))
                .0
                .heatmap
                .unwrap()
        };
        let lit = |heatmap: Heatmap| -> Vec<u8> {
            heatmap
                .days
                .iter()
                .filter(|day| day.value > 0.0)
                .map(|day| day.level)
                .collect()
        };
        assert_eq!(lit(days(json!([60, 20, 40]))), [1, 2, 2, 3, 3, 4, 4, 4]);
        // At 30, 50 and 60: a quarter, half and three quarters of the way.
        assert_eq!(lit(days(json!([5]))), [1, 1, 2, 2, 3, 4, 4, 4]);
    }

    #[test]
    fn a_charts_scale_rounds_up_and_reads_like_the_servers_figures() {
        assert_eq!(scale(0.0), 0.0);
        assert_eq!(scale(1.0), 1.0);
        assert_eq!(scale(1200.0), 2000.0);
        assert_eq!(scale(2000.0), 2000.0);
        assert_eq!(scale(3100.0), 5000.0);
        assert_eq!(scale(7_300_000.0), 10_000_000.0);
        assert!((scale(0.034) - 0.05).abs() < 1e-12);
        let tokens = |v: f64| scale_text(v, Some("tokens"));
        assert_eq!(tokens(500.0), "500");
        assert_eq!(tokens(2000.0), "2k");
        assert_eq!(tokens(10_000_000.0), "10M");
        assert_eq!(tokens(0.5), "0.5");
        let usd = |v: f64| scale_text(v, Some("usd"));
        assert_eq!(usd(5.0), "$5");
        assert_eq!(usd(0.5), "$0.50");
        assert_eq!(usd(0.005), "$0.005");
    }
}
