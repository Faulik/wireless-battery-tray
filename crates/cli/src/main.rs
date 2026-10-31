//! `wbt [--json]` - print the battery of every supported device once.

use battery_core::{Battery, Level};
use serde::Serialize;

#[derive(Serialize)]
struct JsonOut<'a> {
    batteries: &'a [Battery],
    errors: Vec<JsonError<'a>>,
}

#[derive(Serialize)]
struct JsonError<'a> {
    provider: &'a str,
    error: &'a str,
}

fn main() {
    let json = std::env::args().skip(1).any(|a| a == "--json");
    let snap = battery_core::poll_all(&providers::all());

    if json {
        let out = JsonOut { batteries: &snap.batteries, errors: snap.errors.iter().map(|(p, e)| JsonError { provider: p, error: e }).collect() };
        println!("{}", serde_json::to_string_pretty(&out).expect("serialisable"));
        return;
    }

    let rows: Vec<[String; 5]> = snap
        .batteries
        .iter()
        .map(|b| {
            let name = match &b.part {
                Some(p) => format!("{} ({p})", b.name),
                None => b.name.clone(),
            };
            let level = match &b.level {
                Level::Percent(p) if b.connected == Some(false) => format!("{p}% (last known)"),
                Level::Percent(p) => format!("{p}%"),
                Level::Coarse(s) => s.clone(),
                Level::Unavailable(s) => format!("- ({s})"),
            };
            let charging = match b.charging {
                Some(true) => "yes",
                Some(false) => "no",
                None => "?",
            };
            [name, format!("{:?}", b.kind), level, charging.into(), b.source.into()]
        })
        .collect();

    let header = ["DEVICE", "KIND", "BATTERY", "CHARGING", "SOURCE"].map(String::from);
    let mut widths = header.clone().map(|h| h.len());
    for r in &rows {
        for (w, c) in widths.iter_mut().zip(r) {
            *w = (*w).max(c.chars().count());
        }
    }
    for r in std::iter::once(&header).chain(&rows) {
        let line: Vec<String> = r.iter().zip(widths).map(|(c, w)| format!("{c:<w$}")).collect();
        println!("{}", line.join("  ").trim_end());
    }
    for (p, e) in &snap.errors {
        eprintln!("warning: {p}: {e}");
    }
}
