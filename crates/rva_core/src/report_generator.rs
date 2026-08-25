//! Report generator: diff results -> HTML (interactive) / TXT (plain text) / JSON (structured).

use crate::diff_engine::ChangeType;
use crate::diff_engine::DiffEntry;
use crate::symbol_resolver::SymbolMap;
use crate::Result;
use serde::Serialize;

#[derive(Debug, Clone, Default)]
pub struct ReportSummary {
    pub added: u64,
    pub removed: u64,
    pub modified: u64,
    pub total_bytes: u64,
}

/// Unified intermediate model consumed by Report / Patch / GUI.
#[derive(Debug, Clone, Default)]
pub struct DiffReport {
    pub entries: Vec<DiffEntry>,
    pub symbols: Option<SymbolMap>,
    pub summary: ReportSummary,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReportFormat {
    Html,
    Txt,
    Json,
}

impl ReportFormat {
    /// Parse from a CLI string; invalid values fall back to Html.
    pub fn from_str_checked(s: &str) -> Self {
        match s.to_ascii_lowercase().as_str() {
            "txt" => ReportFormat::Txt,
            "json" => ReportFormat::Json,
            _ => ReportFormat::Html,
        }
    }
}

pub trait ReportGenerator {
    fn generate(report: &DiffReport, format: ReportFormat) -> Result<String>;
}

/// Serializable entry mirror: DiffEntry.old/new are Option<Range<u64>>, but std Range does not
/// implement Serialize, so they are mapped to explicit start/end fields for serde_json.
#[derive(Serialize)]
struct JsonEntry {
    offset: u64,
    length: u64,
    change: String,
    old_start: Option<u64>,
    old_end: Option<u64>,
    new_start: Option<u64>,
    new_end: Option<u64>,
}

#[derive(Serialize)]
struct JsonSummary {
    added: u64,
    removed: u64,
    modified: u64,
    total_bytes: u64,
}

#[derive(Serialize)]
struct JsonReport {
    summary: JsonSummary,
    entries: Vec<JsonEntry>,
    symbols: Option<serde_json::Value>,
}

pub struct DefaultReportGenerator;

impl ReportGenerator for DefaultReportGenerator {
    fn generate(report: &DiffReport, format: ReportFormat) -> Result<String> {
        match format {
            ReportFormat::Html => Ok(to_html(report)),
            ReportFormat::Txt => Ok(to_txt(report)),
            ReportFormat::Json => Ok(to_json(report)),
        }
    }
}

fn change_label(c: ChangeType) -> &'static str {
    match c {
        ChangeType::Added => "Added",
        ChangeType::Removed => "Removed",
        ChangeType::Modified => "Modified",
    }
}

fn change_color(c: ChangeType) -> &'static str {
    match c {
        ChangeType::Added => "#2e7d32",     // 绿
        ChangeType::Removed => "#c62828",   // 红
        ChangeType::Modified => "#ef6c00",  // 琥珀
    }
}

fn to_html(report: &DiffReport) -> String {
    let s = &report.summary;
    let mut rows = String::new();
    for e in &report.entries {
        let color = change_color(e.change);
        let label = change_label(e.change);
        rows.push_str(&format!(
            "<tr><td style=\"color:{c};font-weight:600\">{l}</td>\
             <td>0x{off:X}</td><td>{len}</td>\
             <td>{os}</td><td>{ns}</td></tr>\n",
            c = color,
            l = label,
            off = e.offset,
            len = e.length,
            os = range_text(&e.old),
            ns = range_text(&e.new),
        ));
    }
    format!(
        "<!DOCTYPE html>\n<html lang=\"en\">\n<head><meta charset=\"utf-8\">\n\
         <title>rva diff report</title>\n\
         <style>\n\
         body{{font-family:ui-monospace,Menlo,Consolas,monospace;margin:0;background:#0f1115;color:#e6e6e6}}\n\
         .wrap{{max-width:900px;margin:0 auto;padding:24px}}\n\
         h1{{font-size:20px}}\n\
         .cards{{display:flex;gap:12px;margin:16px 0}}\n\
         .card{{flex:1;background:#1a1d24;border:1px solid #2a2e38;border-radius:10px;padding:14px}}\n\
         .card .n{{font-size:24px;font-weight:700}}\n\
         .card.added .n{{color:#2e7d32}}.card.removed .n{{color:#c62828}}.card.modified .n{{color:#ef6c00}}\n\
         table{{width:100%;border-collapse:collapse;font-size:14px}}\n\
         th,td{{text-align:left;padding:6px 10px;border-bottom:1px solid #2a2e38}}\n\
         th{{color:#9aa4b2}}\n\
         </style></head>\n<body><div class=\"wrap\">\n\
         <h1>rva binary diff report</h1>\n\
         <div class=\"cards\">\n\
         <div class=\"card added\"><div>Added</div><div class=\"n\">{a}</div></div>\n\
         <div class=\"card removed\"><div>Removed</div><div class=\"n\">{r}</div></div>\n\
         <div class=\"card modified\"><div>Modified</div><div class=\"n\">{m}</div></div>\n\
         <div class=\"card\"><div>Total bytes</div><div class=\"n\">{t}</div></div>\n\
         </div>\n\
         <table><thead><tr><th>Change</th><th>Offset</th><th>Len</th><th>Old</th><th>New</th></tr></thead>\n\
         <tbody>\n{rows}</tbody></table>\n\
         </div></body></html>",
        a = s.added,
        r = s.removed,
        m = s.modified,
        t = s.total_bytes,
        rows = rows,
    )
}

fn range_text(r: &Option<std::ops::Range<u64>>) -> String {
    match r {
        Some(rg) => format!("0x{:X}..0x{:X}", rg.start, rg.end),
        None => "-".to_string(),
    }
}

fn to_txt(report: &DiffReport) -> String {
    let s = &report.summary;
    let mut out = String::new();
    out.push_str(&format!(
        "Summary: Added={} Removed={} Modified={} TotalBytes={}\n",
        s.added, s.removed, s.modified, s.total_bytes
    ));
    for e in &report.entries {
        out.push_str(&format!(
            "  {} @ off={} len={} old={} new={}\n",
            change_label(e.change),
            e.offset,
            e.length,
            range_text(&e.old),
            range_text(&e.new),
        ));
    }
    out
}

fn to_json(report: &DiffReport) -> String {
    let s = &report.summary;
    let entries: Vec<JsonEntry> = report
        .entries
        .iter()
        .map(|e| JsonEntry {
            offset: e.offset,
            length: e.length,
            change: change_label(e.change).to_string(),
            old_start: e.old.as_ref().map(|r| r.start),
            old_end: e.old.as_ref().map(|r| r.end),
            new_start: e.new.as_ref().map(|r| r.start),
            new_end: e.new.as_ref().map(|r| r.end),
        })
        .collect();
    let symbols = report.symbols.as_ref().map(|sm| {
        serde_json::json!({ "count": sm.0.len() })
    });
    let jr = JsonReport {
        summary: JsonSummary {
            added: s.added,
            removed: s.removed,
            modified: s.modified,
            total_bytes: s.total_bytes,
        },
        entries,
        symbols,
    };
    serde_json::to_string_pretty(&jr).unwrap_or_else(|_| "{}".to_string())
}

/// Summarize a diff by its entries (the CLI fills summary before generating the report).
pub fn summarize(entries: &[DiffEntry]) -> ReportSummary {
    let mut added = 0u64;
    let mut removed = 0u64;
    let mut modified = 0u64;
    let mut total_bytes = 0u64;
    for e in entries {
        total_bytes += e.length;
        match e.change {
            ChangeType::Added => added += e.length,
            ChangeType::Removed => removed += e.length,
            ChangeType::Modified => modified += e.length,
        }
    }
    ReportSummary { added, removed, modified, total_bytes }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ops::Range;

    fn mk(
        change: ChangeType,
        offset: u64,
        length: u64,
        old: Option<Range<u64>>,
        new: Option<Range<u64>>,
    ) -> DiffEntry {
        DiffEntry { offset, length, change, old, new }
    }

    fn sample_report() -> DiffReport {
        let entries = vec![
            mk(ChangeType::Added, 0, 4, None, Some(0..4)),
            mk(ChangeType::Modified, 10, 1, Some(10..11), Some(10..11)),
            mk(ChangeType::Removed, 20, 2, Some(20..22), None),
        ];
        let summary = summarize(&entries);
        DiffReport {
            entries,
            symbols: None,
            summary,
        }
    }

    #[test]
    fn html_contains_root_and_labels() {
        let s = to_html(&sample_report());
        assert!(s.contains("<html") || s.contains("<!DOCTYPE html"));
        assert!(s.contains("Added"));
        assert!(s.contains("Modified"));
        assert!(s.contains("Removed"));
    }

    #[test]
    fn txt_contains_summary_and_added() {
        let s = to_txt(&sample_report());
        assert!(s.contains("Summary"));
        assert!(s.contains("Added"));
        assert!(s.contains("Modified @ off=10"));
    }

    #[test]
    fn json_parses_and_has_summary_entries() {
        let s = to_json(&sample_report());
        let v: serde_json::Value = serde_json::from_str(&s).expect("valid json");
        assert!(v.get("summary").is_some());
        let arr = v.get("entries").expect("entries array").as_array().unwrap();
        assert_eq!(arr.len(), 3);
        assert!(v.get("symbols").is_some());
    }

    #[test]
    fn generate_dispatch_all_formats() {
        let r = sample_report();
        assert!(DefaultReportGenerator::generate(&r, ReportFormat::Html).is_ok());
        assert!(DefaultReportGenerator::generate(&r, ReportFormat::Txt).is_ok());
        assert!(DefaultReportGenerator::generate(&r, ReportFormat::Json).is_ok());
    }
}
