//! The `visualize` tool's report spec: what an agent sends to show charts, tables and KPI tiles
//! in the chat. `Report::parse` is the one strict parser: the MCP tool replies with its error (so
//! the model can fix the call) and the UIs only ever draw specs that passed it. Colors and layout
//! are the UI's business; the spec carries data and intent only.

use serde_json::Value;

pub const MAX_BLOCKS: usize = 12;
const MAX_POINTS: usize = 200;
const MAX_SERIES: usize = 12;
const MAX_ROWS: usize = 500;
const MAX_COLUMNS: usize = 20;
const MAX_ITEMS: usize = 24;

#[derive(Clone, Debug, PartialEq)]
pub struct Report {
    pub title: Option<String>,
    pub subtitle: Option<String>,
    pub blocks: Vec<Block>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Block {
    pub title: Option<String>,
    pub caption: Option<String>,
    /// Columns the block takes in the report's grid, when the agent asked for it.
    pub span: Option<Span>,
    pub kind: Kind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Span {
    Cols(u8),
    Full,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    Stats(Vec<Stat>),
    Bar { labels: Vec<String>, series: Vec<Series>, horizontal: bool, stacked: bool, unit: Option<String> },
    Line { labels: Vec<String>, series: Vec<Series>, area: bool, unit: Option<String> },
    Pie { slices: Vec<Slice>, ring: bool, center: Option<(String, String)>, unit: Option<String> },
    Table { columns: Vec<Column>, rows: Vec<Vec<String>> },
    Progress(Vec<Meter>),
    Text(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Stat {
    pub label: String,
    pub value: String,
    pub delta: Option<String>,
    pub note: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Series {
    pub name: String,
    pub values: Vec<f64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Slice {
    pub label: String,
    pub value: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    Left,
    Right,
    Center,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Column {
    pub label: String,
    pub align: Align,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Meter {
    pub label: String,
    pub value: f64,
    pub max: f64,
}

/// Whether a tool name is winfer's `visualize` as the backends display it ("winfer · visualize",
/// "mcp__winfer__visualize", plain "visualize").
pub fn is_visualize(name: &str) -> bool {
    name == "visualize" || name.ends_with("· visualize") || name.ends_with("__visualize") || name.ends_with("_visualize") && name.contains("winfer")
}

/// The spec inside a tool call's input: backends pass the arguments bare (Claude), under
/// `arguments` (Codex) or `Arguments` (Antigravity).
pub fn spec_of(input: &Value) -> &Value {
    if input.get("blocks").is_some() {
        return input;
    }
    ["arguments", "Arguments", "args"].iter().map(|k| &input[*k]).find(|v| v.get("blocks").is_some()).unwrap_or(input)
}

fn text(v: &Value, key: &str) -> Option<String> {
    match v.get(key)? {
        Value::String(s) if !s.trim().is_empty() => Some(s.trim().to_string()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// A number, also from a string like "12.5", "1,200" or "45%".
fn number(v: &Value) -> Option<f64> {
    let n = match v {
        Value::Number(n) => n.as_f64()?,
        Value::String(s) => s.trim().trim_end_matches('%').replace(',', "").parse().ok()?,
        _ => return None,
    };
    n.is_finite().then_some(n)
}

fn cell(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => if *b { "yes" } else { "no" }.to_string(),
        other => other.to_string(),
    }
}

fn array<'a>(v: &'a Value, key: &str, at: &str, max: usize) -> Result<&'a Vec<Value>, String> {
    let a = v.get(key).and_then(Value::as_array).ok_or_else(|| format!("{at}: `{key}` must be an array"))?;
    if a.is_empty() {
        return Err(format!("{at}: `{key}` is empty"));
    }
    if a.len() > max {
        return Err(format!("{at}: `{key}` has {} entries, at most {max} are allowed; aggregate or keep the top ones", a.len()));
    }
    Ok(a)
}

fn labels(v: &Value, at: &str) -> Result<Vec<String>, String> {
    Ok(array(v, "labels", at, MAX_POINTS)?.iter().map(cell).collect())
}

fn series(v: &Value, at: &str, n: usize) -> Result<Vec<Series>, String> {
    let list = array(v, "series", at, MAX_SERIES)?;
    list.iter()
        .enumerate()
        .map(|(i, s)| {
            let name = text(s, "name").unwrap_or_else(|| if list.len() == 1 { String::new() } else { format!("Series {}", i + 1) });
            let raw = s.get("values").and_then(Value::as_array).ok_or_else(|| format!("{at}: series {} needs a `values` array", i + 1))?;
            if raw.len() != n {
                return Err(format!("{at}: series {} has {} values but there are {n} labels", i + 1, raw.len()));
            }
            let values = raw.iter().map(|x| number(x).ok_or_else(|| format!("{at}: series {} has a non-numeric value ({x})", i + 1))).collect::<Result<_, _>>()?;
            Ok(Series { name, values })
        })
        .collect()
}

fn items<T>(v: &Value, at: &str, f: impl Fn(&Value, &str) -> Result<T, String>) -> Result<Vec<T>, String> {
    array(v, "items", at, MAX_ITEMS)?.iter().enumerate().map(|(i, it)| f(it, &format!("{at} item {}", i + 1))).collect()
}

fn block(v: &Value, i: usize) -> Result<Block, String> {
    let kind_name = v.get("type").and_then(Value::as_str).ok_or_else(|| format!("block {}: missing `type`", i + 1))?;
    let at = format!("block {} ({kind_name})", i + 1);
    let unit = text(v, "unit");
    let kind = match kind_name {
        "stats" => Kind::Stats(items(v, &at, |it, at| {
            Ok(Stat { label: text(it, "label").ok_or_else(|| format!("{at}: needs a `label`"))?, value: text(it, "value").ok_or_else(|| format!("{at}: needs a `value`"))?, delta: text(it, "delta"), note: text(it, "note") })
        })?),
        "bar" | "line" => {
            let labels = labels(v, &at)?;
            let series = series(v, &at, labels.len())?;
            if kind_name == "bar" {
                Kind::Bar { labels, series, horizontal: v.get("orientation").and_then(Value::as_str) != Some("vertical"), stacked: v["stacked"].as_bool().unwrap_or(false), unit }
            } else {
                Kind::Line { labels, series, area: v["area"].as_bool().unwrap_or(false), unit }
            }
        }
        "pie" | "ring" => {
            let slices: Vec<Slice> = array(v, "slices", &at, MAX_POINTS)?
                .iter()
                .enumerate()
                .map(|(i, s)| {
                    let value = s.get("value").and_then(number).ok_or_else(|| format!("{at}: slice {} needs a numeric `value`", i + 1))?;
                    if value < 0.0 {
                        return Err(format!("{at}: slice {} is negative; pie slices are parts of a whole", i + 1));
                    }
                    Ok(Slice { label: text(s, "label").ok_or_else(|| format!("{at}: slice {} needs a `label`", i + 1))?, value })
                })
                .collect::<Result<_, _>>()?;
            if slices.iter().all(|s| s.value == 0.0) {
                return Err(format!("{at}: all slices are 0"));
            }
            let center = v.get("center").and_then(|c| Some((text(c, "value")?, text(c, "label").unwrap_or_default())));
            Kind::Pie { slices, ring: kind_name == "ring" || v["ring"].as_bool().unwrap_or(false), center, unit }
        }
        "table" => {
            let cols = array(v, "columns", &at, MAX_COLUMNS)?;
            let keys: Vec<String> = cols.iter().map(|c| if c.is_string() { cell(c) } else { text(c, "key").or_else(|| text(c, "label")).unwrap_or_default() }).collect();
            let rows_raw = array(v, "rows", &at, MAX_ROWS)?;
            let rows: Vec<Vec<String>> = rows_raw
                .iter()
                .enumerate()
                .map(|(r, row)| match row {
                    Value::Array(cells) if cells.len() == keys.len() => Ok(cells.iter().map(cell).collect()),
                    Value::Array(cells) => Err(format!("{at}: row {} has {} cells but there are {} columns", r + 1, cells.len(), keys.len())),
                    Value::Object(map) => Ok(keys.iter().map(|k| map.get(k).map(cell).unwrap_or_default()).collect()),
                    _ => Err(format!("{at}: row {} must be an array or an object", r + 1)),
                })
                .collect::<Result<_, _>>()?;
            let columns = cols
                .iter()
                .enumerate()
                .map(|(c, col)| {
                    let label = if col.is_string() { cell(col) } else { text(col, "label").or_else(|| text(col, "key")).unwrap_or_default() };
                    let numeric = rows.iter().all(|r| r[c].is_empty() || number(&Value::String(r[c].clone())).is_some());
                    let align = match col.get("align").and_then(Value::as_str) {
                        Some("right") => Align::Right,
                        Some("center") => Align::Center,
                        Some("left") => Align::Left,
                        _ if numeric && rows.iter().any(|r| !r[c].is_empty()) => Align::Right,
                        _ => Align::Left,
                    };
                    Column { label, align }
                })
                .collect();
            Kind::Table { columns, rows }
        }
        "progress" => Kind::Progress(items(v, &at, |it, at| {
            let value = it.get("value").and_then(number).ok_or_else(|| format!("{at}: needs a numeric `value`"))?;
            let max = it.get("max").and_then(number).unwrap_or(100.0);
            if max <= 0.0 {
                return Err(format!("{at}: `max` must be above 0"));
            }
            Ok(Meter { label: text(it, "label").ok_or_else(|| format!("{at}: needs a `label`"))?, value, max })
        })?),
        "text" => Kind::Text(text(v, "markdown").or_else(|| text(v, "text")).ok_or_else(|| format!("{at}: needs `markdown`"))?),
        other => return Err(format!("block {}: unknown type `{other}` (use stats, bar, line, pie, ring, table, progress or text)", i + 1)),
    };
    let span = match v.get("span") {
        Some(Value::String(s)) if s == "full" => Some(Span::Full),
        Some(Value::String(s)) if s == "wide" => Some(Span::Cols(2)),
        Some(n) => number(n).filter(|n| *n >= 1.0).map(|n| if n >= 4.0 { Span::Full } else { Span::Cols(n as u8) }),
        None => None,
    };
    Ok(Block { title: text(v, "title"), caption: text(v, "caption"), span, kind })
}

impl Report {
    pub fn parse(spec: &Value) -> Result<Report, String> {
        let blocks = spec.get("blocks").and_then(Value::as_array).ok_or("`blocks` must be an array of charts, tables and so on")?;
        if blocks.is_empty() {
            return Err("`blocks` is empty".into());
        }
        if blocks.len() > MAX_BLOCKS {
            return Err(format!("{} blocks; at most {MAX_BLOCKS} per report (call visualize again for more)", blocks.len()));
        }
        Ok(Report { title: text(spec, "title"), subtitle: text(spec, "subtitle"), blocks: blocks.iter().enumerate().map(|(i, b)| block(b, i)).collect::<Result<_, _>>()? })
    }

    /// What the tool tells the model: the user now sees this.
    pub fn summary(&self) -> String {
        let mut counts: Vec<(&str, usize)> = Vec::new();
        for b in &self.blocks {
            let name = b.kind.name();
            match counts.iter_mut().find(|(n, _)| *n == name) {
                Some((_, c)) => *c += 1,
                None => counts.push((name, 1)),
            }
        }
        let parts: Vec<String> = counts.iter().map(|(n, c)| format!("{c} {n}{}", if *c > 1 { "s" } else { "" })).collect();
        format!("Shown to the user in the chat: {}. Don't repeat the numbers in text; add what they mean.", parts.join(", "))
    }
}

impl Kind {
    pub fn name(&self) -> &'static str {
        match self {
            Kind::Stats(_) => "stat row",
            Kind::Bar { .. } => "bar chart",
            Kind::Line { area: true, .. } => "area chart",
            Kind::Line { .. } => "line chart",
            Kind::Pie { ring: true, .. } => "ring chart",
            Kind::Pie { .. } => "pie chart",
            Kind::Table { .. } => "table",
            Kind::Progress(_) => "progress list",
            Kind::Text(_) => "text block",
        }
    }
}

fn trim_num(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 1e15 { format!("{}", n as i64) } else { format!("{n}") }
}

fn csv_cell(s: &str) -> String {
    if s.contains([',', '"', '\n']) { format!("\"{}\"", s.replace('"', "\"\"")) } else { s.to_string() }
}

fn csv(rows: Vec<Vec<String>>) -> String {
    rows.iter().map(|r| r.iter().map(|c| csv_cell(c)).collect::<Vec<_>>().join(",")).collect::<Vec<_>>().join("\n")
}

impl Block {
    /// The block's data as CSV (or the text itself), for "Copy data".
    pub fn to_csv(&self) -> String {
        match &self.kind {
            Kind::Stats(s) => csv(std::iter::once(vec!["label".into(), "value".into(), "change".into()]).chain(s.iter().map(|x| vec![x.label.clone(), x.value.clone(), x.delta.clone().unwrap_or_default()])).collect()),
            Kind::Bar { labels, series, .. } | Kind::Line { labels, series, .. } => {
                let head = std::iter::once(String::new()).chain(series.iter().map(|s| s.name.clone())).collect();
                csv(std::iter::once(head).chain(labels.iter().enumerate().map(|(i, l)| std::iter::once(l.clone()).chain(series.iter().map(|s| trim_num(s.values[i]))).collect())).collect())
            }
            Kind::Pie { slices, .. } => csv(std::iter::once(vec!["label".into(), "value".into()]).chain(slices.iter().map(|s| vec![s.label.clone(), trim_num(s.value)])).collect()),
            Kind::Table { columns, rows } => csv(std::iter::once(columns.iter().map(|c| c.label.clone()).collect()).chain(rows.iter().cloned()).collect()),
            Kind::Progress(m) => csv(std::iter::once(vec!["label".into(), "value".into(), "max".into()]).chain(m.iter().map(|x| vec![x.label.clone(), trim_num(x.value), trim_num(x.max)])).collect()),
            Kind::Text(t) => t.clone(),
        }
    }
}

/// The `visualize` tool's JSON schema and description.
pub fn tool() -> Value {
    use serde_json::json;
    json!({
        "name": "visualize",
        "description": "Show the user a report in the chat: KPI tiles, bar/line/area charts, pie or ring charts, tables, progress meters and text, rendered natively. Use it instead of ASCII tables or long number lists when you summarize, compare or analyze data (usage, benchmarks, test results, file sizes, costs, timelines). One call can hold several blocks: they lay out in a grid of 2-4 columns when the chat is wide enough (stats and text take the full width, tables two columns; set `span` to change that). Calls made in a row are merged into one report, but prefer one call with all blocks. Keep it focused: few series, short labels, at most ~7 pie slices. Don't pick colors; winfer themes them. Then add a sentence on what the data means, not the numbers again. Example: {\"title\":\"Build times\",\"blocks\":[{\"type\":\"stats\",\"items\":[{\"label\":\"Median\",\"value\":\"41s\",\"delta\":\"-8%\"}]},{\"type\":\"bar\",\"title\":\"Seconds per crate\",\"labels\":[\"core\",\"ui\"],\"series\":[{\"name\":\"build\",\"values\":[12,29]}]},{\"type\":\"pie\",\"ring\":true,\"slices\":[{\"label\":\"Rust\",\"value\":80},{\"label\":\"TS\",\"value\":20}]},{\"type\":\"table\",\"columns\":[\"Crate\",\"Time\"],\"rows\":[[\"core\",12],[\"ui\",29]]}]}",
        "inputSchema": {
            "type": "object",
            "properties": {
                "title": { "type": "string" },
                "subtitle": { "type": "string" },
                "blocks": {
                    "type": "array",
                    "description": format!("1-{MAX_BLOCKS} blocks, each an object with `type` and optional `title`, `caption` and `span` (columns: 1-4 or \"full\"). stats: items[{{label, value, delta?, note?}}]. bar: labels[], series[{{name, values[]}}], orientation? (horizontal default | vertical), stacked?, unit?. line: labels[], series[{{name, values[]}}], area?, unit?. pie: slices[{{label, value}}], ring? (donut), center? {{value, label}}, unit?. table: columns[strings or {{key, label, align?}}], rows[arrays or objects]. progress: items[{{label, value, max? (100)}}]. text: markdown."),
                    "items": { "type": "object", "properties": { "type": { "type": "string", "enum": ["stats", "bar", "line", "pie", "ring", "table", "progress", "text"] } }, "required": ["type"] }
                }
            },
            "required": ["blocks"]
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn parse(blocks: Value) -> Result<Report, String> {
        Report::parse(&json!({ "blocks": blocks }))
    }

    #[test]
    fn a_full_report_parses_and_is_summarized() {
        let r = Report::parse(&json!({ "title": "T", "blocks": [
            { "type": "stats", "items": [{ "label": "A", "value": 3, "delta": "+1%" }] },
            { "type": "bar", "labels": ["a", "b"], "series": [{ "name": "x", "values": [1, "1,200"] }] },
            { "type": "ring", "slices": [{ "label": "a", "value": 1 }, { "label": "b", "value": 3 }] },
            { "type": "table", "columns": ["Name", { "key": "n", "label": "N" }], "rows": [["a", 1], { "Name": "b", "n": 22 }] },
            { "type": "progress", "span": "full", "items": [{ "label": "disk", "value": 40 }] },
        ] })).unwrap();
        assert_eq!(r.blocks[4].span, Some(Span::Full));
        assert_eq!(r.blocks[0].span, None);
        assert_eq!(r.blocks.len(), 5);
        assert!(matches!(&r.blocks[1].kind, Kind::Bar { horizontal: true, series, .. } if series[0].values == vec![1.0, 1200.0]));
        assert!(matches!(&r.blocks[3].kind, Kind::Table { columns, rows } if columns[1].align == Align::Right && columns[0].align == Align::Left && rows[1] == vec!["b", "22"]));
        assert_eq!(r.summary(), "Shown to the user in the chat: 1 stat row, 1 bar chart, 1 ring chart, 1 table, 1 progress list. Don't repeat the numbers in text; add what they mean.");
    }

    #[test]
    fn mistakes_come_back_as_readable_errors() {
        assert!(parse(json!([])).unwrap_err().contains("empty"));
        assert!(parse(json!([{ "type": "radar" }])).unwrap_err().contains("unknown type `radar`"));
        assert!(parse(json!([{ "type": "bar", "labels": ["a", "b"], "series": [{ "name": "x", "values": [1] }] }])).unwrap_err().contains("1 values but there are 2 labels"));
        assert!(parse(json!([{ "type": "pie", "slices": [{ "label": "a", "value": -1 }] }])).unwrap_err().contains("negative"));
        assert!(parse(json!([{ "type": "pie", "slices": [{ "label": "a", "value": 0 }] }])).unwrap_err().contains("all slices are 0"));
        assert!(parse(json!([{ "type": "table", "columns": ["a", "b"], "rows": [[1]] }])).unwrap_err().contains("1 cells but there are 2 columns"));
        assert!(parse(json!([{ "type": "bar", "labels": ["a"], "series": [{ "values": ["x"] }] }])).unwrap_err().contains("non-numeric"));
        assert!(Report::parse(&json!({})).is_err());
        assert!(parse(Value::Array(vec![json!({ "type": "text", "markdown": "x" }); MAX_BLOCKS + 1])).unwrap_err().contains("at most"));
    }

    #[test]
    fn data_copies_as_csv() {
        let r = parse(json!([{ "type": "line", "labels": ["Mon", "Tue"], "series": [{ "name": "a,b", "values": [1, 2.5] }] }])).unwrap();
        assert_eq!(r.blocks[0].to_csv(), ",\"a,b\"\nMon,1\nTue,2.5");
    }

    #[test]
    fn finds_the_tool_under_every_display_name() {
        for n in ["visualize", "winfer · visualize", "mcp__winfer__visualize", "mcp_winfer_visualize"] {
            assert!(is_visualize(n), "{n}");
        }
        assert!(!is_visualize("winfer · bg"));
        assert!(!is_visualize("Bash"));
    }
}
