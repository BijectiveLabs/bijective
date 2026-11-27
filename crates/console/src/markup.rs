// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The one place markup is made: escaping, the page frame, the strip and the navigation.
//!
//! Venue text reaches the console and none of it is ours, so every panel escapes through `text`.
//! The page loads nothing from a network and runs no script. Its policy forbids both.

use crate::panel::{Level, Signal};

/// Escape text between tags.
#[must_use]
pub fn text(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for c in raw.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

/// A money figure at the platform's money scale (eight decimals), shown to two decimals, cut
/// toward zero so a display never states more than was recorded.
#[must_use]
pub fn money(scaled: i128) -> String {
    const UNIT: i128 = 100_000_000;
    let sign = if scaled < 0 { "-" } else { "" };
    let magnitude = scaled.unsigned_abs();
    let unit = UNIT.unsigned_abs();
    format!("{sign}{}.{:02}", magnitude / unit, (magnitude % unit) / (unit / 100))
}

/// A scaled integer as the decimal it stands for, every recorded digit shown and none added.
#[must_use]
pub fn decimal(scaled: impl Into<i128>, scale: u8) -> String {
    let scaled: i128 = scaled.into();
    let sign = if scaled < 0 { "-" } else { "" };
    let digits = scaled.unsigned_abs().to_string();
    let scale = usize::from(scale);
    if scale == 0 {
        return format!("{sign}{digits}");
    }
    let padded = format!("{digits:0>width$}", width = scale + 1);
    let (whole, fraction) = padded.split_at(padded.len() - scale);
    format!("{sign}{whole}.{fraction}")
}

const SPARK_WIDTH: i128 = 240;
const SPARK_HEIGHT: i128 = 36;

/// A line of `values`, oldest first, scaled to its own range. Empty below two points: one point
/// draws no trend, and inventing a second would.
#[must_use]
pub fn sparkline(values: &[i128]) -> String {
    if values.len() < 2 {
        return String::new();
    }
    let low = values.iter().copied().min().unwrap_or(0);
    let high = values.iter().copied().max().unwrap_or(0);
    let span = (high - low).max(1);
    let last = i128::try_from(values.len() - 1).unwrap_or(1).max(1);
    let points: Vec<String> = values
        .iter()
        .enumerate()
        .map(|(i, v)| {
            let x = i128::try_from(i).unwrap_or(0) * SPARK_WIDTH / last;
            let y = SPARK_HEIGHT - (v - low) * SPARK_HEIGHT / span;
            format!("{x},{y}")
        })
        .collect();
    format!(
        "<svg viewBox=\"0 0 {SPARK_WIDTH} {SPARK_HEIGHT}\" width=\"{SPARK_WIDTH}\" \
         height=\"{SPARK_HEIGHT}\" role=\"img\"><polyline fill=\"none\" stroke=\"#335\" \
         stroke-width=\"1.5\" points=\"{}\"/></svg>",
        points.join(" ")
    )
}

/// A table head from static column names, which are this build's and need no escaping.
pub fn table_head(out: &mut String, columns: &[&'static str]) {
    out.push_str("<table><tr>");
    for column in columns {
        out.push_str("<th>");
        out.push_str(column);
    }
    out.push_str("</tr>");
}

/// The line a panel shows when it has nothing to show, so an empty view reads as empty.
pub fn nothing(out: &mut String, sentence: &'static str) {
    out.push_str("<p class=\"none\">");
    out.push_str(sentence);
    out.push_str("</p>");
}

const STYLE: &str = "body{font:14px/1.5 ui-monospace,monospace;margin:0;color:#111}\
main{margin:1.5rem 2rem;max-width:90rem}h1{font-size:1.1rem;margin:0}\
h2{font-size:.95rem;margin-top:2rem;border-bottom:1px solid #999}\
header{position:sticky;top:0;background:#f4f4f4;border-bottom:1px solid #bbb;padding:.6rem 2rem}\
nav a{margin-right:1.2rem}nav a.here{font-weight:700;text-decoration:none;color:#111}\
.strip{display:flex;flex-wrap:wrap;gap:.4rem 1.6rem;margin:.4rem 0}.strip span{white-space:nowrap}\
.attention{color:#a15c00}.alarm{color:#a10000;font-weight:700}\
table{border-collapse:collapse;width:100%}td,th{text-align:left;padding:.2rem .6rem .2rem 0;\
vertical-align:top}th{font-weight:600}em{color:#a15c00;font-style:normal}\
strong{color:#a10000}.none{color:#666}";

/// The page frame around one group's panels: the strip and the navigation are on every page.
pub fn page(
    title: &str,
    here: &str,
    nav: &[(&'static str, &'static str)],
    signals: &[Signal],
    body: &str,
) -> String {
    let mut out = String::new();
    out.push_str("<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">");
    out.push_str("<meta http-equiv=\"refresh\" content=\"10\">");
    out.push_str(&format!("<title>Bijective console - {}</title><style>", text(title)));
    out.push_str(STYLE);
    out.push_str("</style></head><body><header><h1>Bijective console</h1>");
    strip(&mut out, signals);
    out.push_str("<nav>");
    for (route, label) in nav {
        let class = if *route == here { " class=\"here\"" } else { "" };
        out.push_str(&format!("<a href=\"/{route}\"{class}>{label}</a>"));
    }
    out.push_str("</nav></header><main>");
    out.push_str(body);
    out.push_str("</main></body></html>");
    out
}

fn strip(out: &mut String, signals: &[Signal]) {
    out.push_str("<div class=\"strip\">");
    for signal in signals {
        let class = match signal.level {
            Level::Normal => "",
            Level::Attention => " class=\"attention\"",
            Level::Alarm => " class=\"alarm\"",
        };
        out.push_str(&format!("<span{class}>{}: {}</span>", signal.label, text(&signal.value)));
    }
    out.push_str("</div>");
}
