use agent_client_protocol::schema::v1::{
    ContentBlock, ContentChunk, ImageContent, ResourceLink, TextContent,
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::fmt::Write as _;
use thiserror::Error;

pub const RICH_CONTENT_NAMESPACE: &str = "longbridge.ai/rich-content";
pub const CHART_MIME_TYPE: &str = "application/vnd.longbridge.chart+json";
pub const TABLE_MIME_TYPE: &str = "application/vnd.longbridge.table+json";
pub const RICH_CONTENT_VERSION: u8 = 1;

const CHART_TYPES: &[&str] = &[
    "line",
    "area",
    "bar",
    "column",
    "pie",
    "scatter",
    "histogram",
    "treemap",
    "word-cloud",
    "dual-axes",
    "radar",
    "pin-map",
    "path-map",
    "heat-map",
    "mind-map",
    "fishbone-diagram",
    "flow-diagram",
    "indented-tree",
    "network-graph",
    "organization-chart",
    "vis-text",
    "funnel",
    "boxplot",
    "sankey",
];

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RichContentKind {
    Chart,
    Table,
    Svg,
    Html,
    Widget,
    Artifact,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RichContent {
    pub version: u8,
    pub content_id: String,
    pub kind: RichContentKind,
    pub mime_type: String,
    pub data: Value,
    pub fallback: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub svg: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Table {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

#[derive(Debug, Error, Eq, PartialEq)]
pub enum RichContentError {
    #[error("rich content id cannot be empty")]
    EmptyContentId,
    #[error("chart configuration must be a JSON object")]
    InvalidChart,
    #[error("chart type is missing")]
    MissingChartType,
    #[error("unsupported chart type: {0}")]
    UnsupportedChartType(String),
    #[error("table must contain at least one column")]
    EmptyTable,
    #[error("table row {row} has {actual} cells, expected {expected}")]
    InvalidTableRow {
        row: usize,
        expected: usize,
        actual: usize,
    },
    #[error("SVG contains active or external content")]
    UnsafeSvg,
    #[error("widget URI must use the widget scheme and contain no control characters")]
    InvalidWidgetUri,
}

impl RichContent {
    pub fn chart(content_id: impl Into<String>, mut data: Value) -> Result<Self, RichContentError> {
        let content_id = checked_id(content_id.into())?;
        let object = data.as_object_mut().ok_or(RichContentError::InvalidChart)?;
        let raw_type = object
            .get("type")
            .and_then(Value::as_str)
            .ok_or(RichContentError::MissingChartType)?;
        let chart_type = normalize_chart_type(raw_type);
        if !CHART_TYPES.contains(&chart_type.as_str()) {
            return Err(RichContentError::UnsupportedChartType(raw_type.to_owned()));
        }
        object.insert("type".to_owned(), Value::String(chart_type));
        let fallback = chart_markdown_fallback(&data);
        let svg = render_chart_svg(&data);
        Ok(Self {
            version: RICH_CONTENT_VERSION,
            content_id,
            kind: RichContentKind::Chart,
            mime_type: CHART_MIME_TYPE.to_owned(),
            data,
            fallback,
            svg,
        })
    }

    pub fn table(content_id: impl Into<String>, table: &Table) -> Result<Self, RichContentError> {
        let content_id = checked_id(content_id.into())?;
        validate_table(table)?;
        let fallback = table.to_markdown();
        Ok(Self {
            version: RICH_CONTENT_VERSION,
            content_id,
            kind: RichContentKind::Table,
            mime_type: TABLE_MIME_TYPE.to_owned(),
            data: serde_json::to_value(table).expect("table always serializes"),
            fallback,
            svg: None,
        })
    }

    pub fn opaque(
        content_id: impl Into<String>,
        kind: RichContentKind,
        mime_type: impl Into<String>,
        data: Value,
        fallback: impl Into<String>,
    ) -> Result<Self, RichContentError> {
        Ok(Self {
            version: RICH_CONTENT_VERSION,
            content_id: checked_id(content_id.into())?,
            kind,
            mime_type: mime_type.into(),
            data,
            fallback: fallback.into(),
            svg: None,
        })
    }

    pub fn svg(
        content_id: impl Into<String>,
        svg: impl Into<String>,
        fallback_label: impl AsRef<str>,
    ) -> Result<Self, RichContentError> {
        let svg = svg.into();
        if !is_safe_svg(&svg) {
            return Err(RichContentError::UnsafeSvg);
        }
        let label = fallback_label.as_ref();
        Ok(Self {
            version: RICH_CONTENT_VERSION,
            content_id: checked_id(content_id.into())?,
            kind: RichContentKind::Svg,
            mime_type: "image/svg+xml".to_owned(),
            data: Value::String(svg.clone()),
            fallback: format!("```svg-inline\n{svg}\n```\n\n{label}"),
            svg: Some(svg),
        })
    }

    pub fn widget(
        content_id: impl Into<String>,
        uri: impl Into<String>,
        fallback: impl Into<String>,
    ) -> Result<Self, RichContentError> {
        let uri = uri.into();
        if !uri.starts_with("widget://")
            || uri.len() > 2_048
            || uri.chars().any(char::is_control)
            || uri.contains(['<', '>', '"', '\'', '`'])
        {
            return Err(RichContentError::InvalidWidgetUri);
        }
        Ok(Self {
            version: RICH_CONTENT_VERSION,
            content_id: checked_id(content_id.into())?,
            kind: RichContentKind::Widget,
            mime_type: "application/vnd.longbridge.widget+uri".to_owned(),
            data: serde_json::json!({ "uri": uri }),
            fallback: fallback.into(),
            svg: None,
        })
    }

    #[must_use]
    pub fn to_acp_chunks(&self) -> Vec<ContentChunk> {
        self.to_acp_chunks_with_meta(None)
    }

    #[must_use]
    pub fn to_acp_chunks_with_meta(&self, extra: Option<&Map<String, Value>>) -> Vec<ContentChunk> {
        let metadata = self.metadata();
        let metadata = merge_metadata(metadata, extra);
        let text = ContentBlock::Text(TextContent::new(&self.fallback).meta(metadata.clone()));
        let mut chunks = vec![ContentChunk::new(text)];
        if let Some(svg) = &self.svg {
            let image = ImageContent::new(STANDARD.encode(svg), "image/svg+xml")
                .uri(format!("longbridge-rich://{}/preview.svg", self.content_id))
                .meta(metadata.clone());
            chunks.push(ContentChunk::new(ContentBlock::Image(image)));
        }
        if self.kind == RichContentKind::Widget {
            if let Some(uri) = self.data.get("uri").and_then(Value::as_str) {
                let resource = ResourceLink::new(widget_title(uri), uri)
                    .mime_type(&self.mime_type)
                    .description(&self.fallback)
                    .meta(metadata);
                chunks.push(ContentChunk::new(ContentBlock::ResourceLink(resource)));
            }
        }
        chunks
    }

    #[must_use]
    pub fn svg_preview_chunk(&self) -> Option<ContentChunk> {
        let svg = self.svg.as_ref()?;
        let image = ImageContent::new(STANDARD.encode(svg), "image/svg+xml")
            .uri(format!("longbridge-rich://{}/preview.svg", self.content_id))
            .meta(self.metadata());
        Some(ContentChunk::new(ContentBlock::Image(image)))
    }

    #[must_use]
    pub fn metadata(&self) -> Map<String, Value> {
        let mut metadata = Map::new();
        metadata.insert(
            RICH_CONTENT_NAMESPACE.to_owned(),
            serde_json::to_value(self).expect("rich content always serializes"),
        );
        metadata
    }
}

fn merge_metadata(
    mut metadata: Map<String, Value>,
    extra: Option<&Map<String, Value>>,
) -> Map<String, Value> {
    if let Some(extra) = extra {
        metadata.extend(extra.clone());
    }
    metadata
}

fn widget_title(uri: &str) -> &'static str {
    if uri.starts_with("widget://quote/security/comparison") {
        "Security comparison"
    } else if uri.starts_with("widget://quote/security") {
        "Security quote"
    } else if uri.starts_with("widget://stock/list") {
        "Security list"
    } else {
        "Longbridge interactive content"
    }
}

impl Table {
    #[must_use]
    pub fn to_markdown(&self) -> String {
        let mut output = String::new();
        if let Some(title) = self.title.as_deref().filter(|title| !title.is_empty()) {
            output.push_str("### ");
            output.push_str(title);
            output.push_str("\n\n");
        }
        output.push('|');
        for column in &self.columns {
            output.push(' ');
            output.push_str(&escape_markdown_cell(column));
            output.push_str(" |");
        }
        output.push_str("\n|");
        for _ in &self.columns {
            output.push_str(" --- |");
        }
        for row in &self.rows {
            output.push_str("\n|");
            for cell in row {
                output.push(' ');
                output.push_str(&escape_markdown_cell(cell));
                output.push_str(" |");
            }
        }
        output
    }
}

#[must_use]
pub fn supported_chart_types() -> &'static [&'static str] {
    CHART_TYPES
}

#[must_use]
pub fn normalize_chart_type(value: &str) -> String {
    match value.trim().to_ascii_lowercase().replace('_', "-").as_str() {
        "dualaxes" => "dual-axes".to_owned(),
        "wordcloud" => "word-cloud".to_owned(),
        "pinmap" => "pin-map".to_owned(),
        "pathmap" => "path-map".to_owned(),
        "heatmap" => "heat-map".to_owned(),
        "mindmap" => "mind-map".to_owned(),
        "fishbone" => "fishbone-diagram".to_owned(),
        "flow" => "flow-diagram".to_owned(),
        "indentedtree" => "indented-tree".to_owned(),
        "network" => "network-graph".to_owned(),
        "organization" => "organization-chart".to_owned(),
        "text" => "vis-text".to_owned(),
        value => value.to_owned(),
    }
}

#[must_use]
pub fn charts_from_markdown(markdown: &str, content_id_prefix: &str) -> Vec<RichContent> {
    let mut charts = Vec::new();
    let mut rest = markdown;
    while let Some(open) = rest.find("```vis-chart") {
        let after_open = &rest[open + "```vis-chart".len()..];
        let Some(body) = after_open
            .strip_prefix("\r\n")
            .or_else(|| after_open.strip_prefix('\n'))
        else {
            rest = after_open;
            continue;
        };
        let Some(close) = body.find("```") else {
            break;
        };
        if let Ok(data) = serde_json::from_str::<Value>(body[..close].trim()) {
            let id = format!("{content_id_prefix}:chart-{}", charts.len() + 1);
            if let Ok(chart) = RichContent::chart(id, data) {
                charts.push(chart);
            }
        }
        rest = &body[close + 3..];
    }
    charts
}

fn checked_id(content_id: String) -> Result<String, RichContentError> {
    if content_id.trim().is_empty() {
        Err(RichContentError::EmptyContentId)
    } else {
        Ok(content_id)
    }
}

fn validate_table(table: &Table) -> Result<(), RichContentError> {
    if table.columns.is_empty() {
        return Err(RichContentError::EmptyTable);
    }
    for (index, row) in table.rows.iter().enumerate() {
        if row.len() != table.columns.len() {
            return Err(RichContentError::InvalidTableRow {
                row: index,
                expected: table.columns.len(),
                actual: row.len(),
            });
        }
    }
    Ok(())
}

fn chart_markdown_fallback(data: &Value) -> String {
    let title = data.get("title").and_then(Value::as_str);
    let rows = data.get("data").and_then(Value::as_array);
    let Some(rows) = rows.filter(|rows| !rows.is_empty()) else {
        return title.map_or_else(
            || "Chart data is unavailable.".to_owned(),
            |title| format!("### {title}\n\nChart data is unavailable."),
        );
    };

    let mut columns = Vec::<String>::new();
    for row in rows {
        if let Some(object) = row.as_object() {
            for key in object.keys() {
                if !columns.contains(key) {
                    columns.push(key.clone());
                }
            }
        }
    }
    if columns.is_empty() {
        return title.map_or_else(
            || "Chart contains non-tabular data.".to_owned(),
            |title| format!("### {title}\n\nChart contains non-tabular data."),
        );
    }

    let table = Table {
        columns: columns.clone(),
        rows: rows
            .iter()
            .map(|row| {
                columns
                    .iter()
                    .map(|column| row.get(column).map(value_to_cell).unwrap_or_default())
                    .collect()
            })
            .collect(),
        title: title.map(ToOwned::to_owned),
    };
    table.to_markdown()
}

fn value_to_cell(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(value) => value.clone(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        value => serde_json::to_string(value).expect("JSON values always serialize"),
    }
}

fn escape_markdown_cell(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('|', "\\|")
        .replace(['\r', '\n'], "<br>")
}

fn render_chart_svg(data: &Value) -> Option<String> {
    let chart_type = data.get("type")?.as_str()?;
    let items = data.get("data")?.as_array()?;
    let points = chart_points(items);
    if points.is_empty() {
        return None;
    }
    if chart_type == "column" && has_multiple_groups(items) {
        return Some(render_grouped_columns(data, items));
    }
    match chart_type {
        "column" | "histogram" => Some(render_columns(data, &points)),
        "bar" => Some(render_bars(data, &points)),
        "line" | "area" | "scatter" => Some(render_plot(data, &points, chart_type)),
        "pie" => Some(render_pie(data, &points)),
        _ => None,
    }
}

fn has_multiple_groups(items: &[Value]) -> bool {
    let mut groups = Vec::new();
    for group in items
        .iter()
        .filter_map(|item| item.get("group").and_then(value_label))
    {
        if !groups.contains(&group) {
            groups.push(group);
        }
    }
    groups.len() > 1
}

fn render_grouped_columns(data: &Value, items: &[Value]) -> String {
    let mut categories = Vec::new();
    let mut groups = Vec::new();
    let mut values = Map::new();
    let mut max = 1.0_f64;
    for item in items {
        let Some(category) = item.get("category").and_then(value_label) else {
            continue;
        };
        let Some(group) = item.get("group").and_then(value_label) else {
            continue;
        };
        let Some(value) = item.get("value").and_then(value_number) else {
            continue;
        };
        if !categories.contains(&category) {
            categories.push(category.clone());
        }
        if !groups.contains(&group) {
            groups.push(group.clone());
        }
        max = max.max(value.abs());
        values.insert(format!("{category}\0{group}"), Value::from(value));
    }
    let colors = ["#16a3a5", "#2563eb", "#f59e0b", "#ef4444", "#8b5cf6"];
    let category_width = 600.0 / usize_as_f64(categories.len().max(1));
    let bar_width = (category_width - 16.0) / usize_as_f64(groups.len().max(1));
    let mut body = String::from(r#"<line class="grid" x1="60" y1="390" x2="680" y2="390"/>"#);
    for (category_index, category) in categories.iter().enumerate() {
        for (group_index, group) in groups.iter().enumerate() {
            let value = values
                .get(&format!("{category}\0{group}"))
                .and_then(Value::as_f64)
                .unwrap_or_default();
            let height = value.abs() / max * 290.0;
            let x = 70.0
                + usize_as_f64(category_index) * category_width
                + usize_as_f64(group_index) * bar_width;
            let y = 390.0 - height;
            write!(
                body,
                r#"<rect x="{x:.1}" y="{y:.1}" width="{:.1}" height="{height:.1}" fill="{}"/>"#,
                (bar_width - 3.0).max(2.0),
                colors[group_index % colors.len()]
            )
            .expect("writing to a String cannot fail");
        }
        let label_x = 70.0 + usize_as_f64(category_index) * category_width + category_width / 2.0;
        write!(
            body,
            r#"<text x="{label_x:.1}" y="415" text-anchor="middle">{}</text>"#,
            xml_escape(category)
        )
        .expect("writing to a String cannot fail");
    }
    for (index, group) in groups.iter().enumerate() {
        write!(body, r#"<rect x="700" y="{}" width="12" height="12" fill="{}"/><text x="718" y="{}">{}</text>"#, 70 + index * 25, colors[index % colors.len()], 81 + index * 25, xml_escape(group)).expect("writing to a String cannot fail");
    }
    svg_shell(data, &body)
}

fn chart_points(items: &[Value]) -> Vec<(String, f64)> {
    items
        .iter()
        .filter_map(|item| {
            let object = item.as_object()?;
            let label = ["category", "time", "x", "name", "label", "date"]
                .iter()
                .find_map(|key| object.get(*key).and_then(value_label))?;
            let value = ["value", "y", "count"]
                .iter()
                .find_map(|key| object.get(*key).and_then(value_number))?;
            value.is_finite().then_some((label, value))
        })
        .collect()
}

fn value_label(value: &Value) -> Option<String> {
    match value {
        Value::String(value) => Some(value.split_whitespace().collect::<Vec<_>>().join(" ")),
        Value::Number(value) => Some(value.to_string()),
        _ => None,
    }
}

fn value_number(value: &Value) -> Option<f64> {
    match value {
        Value::Number(value) => value.as_f64(),
        Value::String(value) => value.trim().parse().ok(),
        _ => None,
    }
}

fn svg_shell(data: &Value, body: &str) -> String {
    let title = data
        .get("title")
        .and_then(Value::as_str)
        .map(xml_escape)
        .unwrap_or_default();
    format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" role="img" aria-label="{title}" viewBox="0 0 800 450"><style>text{{font:14px system-ui,sans-serif;fill:#334155}}.title{{font-size:20px;font-weight:600;fill:#0f172a}}.grid{{stroke:#e2e8f0}}.mark{{fill:#16a3a5}}.line{{fill:none;stroke:#16a3a5;stroke-width:3}}</style><text class="title" x="50" y="35">{title}</text>{body}</svg>"#
    )
}

fn render_columns(data: &Value, points: &[(String, f64)]) -> String {
    let max = points
        .iter()
        .map(|(_, value)| value.abs())
        .fold(0.0, f64::max)
        .max(1.0);
    let width = 680.0 / usize_as_f64(points.len());
    let mut body = String::from(r#"<line class="grid" x1="60" y1="390" x2="760" y2="390"/>"#);
    for (index, (label, value)) in points.iter().enumerate() {
        let height = value.abs() / max * 300.0;
        let x = 70.0 + usize_as_f64(index) * width;
        let y = 390.0 - height;
        write!(body, r#"<rect class="mark" x="{x:.1}" y="{y:.1}" width="{:.1}" height="{height:.1}"/><text x="{:.1}" y="415" text-anchor="middle">{}</text>"#, (width - 12.0).max(2.0), x + (width - 12.0).max(2.0) / 2.0, xml_escape(label)).expect("writing to a String cannot fail");
    }
    svg_shell(data, &body)
}

fn render_bars(data: &Value, points: &[(String, f64)]) -> String {
    let max = points
        .iter()
        .map(|(_, value)| value.abs())
        .fold(0.0, f64::max)
        .max(1.0);
    let height = 330.0 / usize_as_f64(points.len());
    let mut body = String::new();
    for (index, (label, value)) in points.iter().enumerate() {
        let width = value.abs() / max * 560.0;
        let y = 60.0 + usize_as_f64(index) * height;
        write!(body, r#"<text x="150" y="{:.1}" text-anchor="end">{}</text><rect class="mark" x="165" y="{y:.1}" width="{width:.1}" height="{:.1}"/>"#, y + height * 0.55, xml_escape(label), (height - 8.0).max(2.0)).expect("writing to a String cannot fail");
    }
    svg_shell(data, &body)
}

fn render_plot(data: &Value, points: &[(String, f64)], chart_type: &str) -> String {
    let min = points
        .iter()
        .map(|(_, value)| *value)
        .fold(f64::INFINITY, f64::min);
    let max = points
        .iter()
        .map(|(_, value)| *value)
        .fold(f64::NEG_INFINITY, f64::max);
    let range = (max - min).max(1.0);
    let step = 680.0 / usize_as_f64(points.len().saturating_sub(1).max(1));
    let coordinates = points
        .iter()
        .enumerate()
        .map(|(index, (_, value))| {
            (
                60.0 + usize_as_f64(index) * step,
                390.0 - (*value - min) / range * 300.0,
            )
        })
        .collect::<Vec<_>>();
    let path = coordinates
        .iter()
        .enumerate()
        .map(|(index, (x, y))| format!("{} {x:.1} {y:.1}", if index == 0 { "M" } else { "L" }))
        .collect::<Vec<_>>()
        .join(" ");
    let mut body = if chart_type == "area" {
        format!(
            r##"<path d="{path} L 740 390 L 60 390 Z" fill="#16a3a5" fill-opacity=".18"/><path class="line" d="{path}"/>"##
        )
    } else if chart_type == "line" {
        format!(r#"<path class="line" d="{path}"/>"#)
    } else {
        String::new()
    };
    for (index, ((label, _), (x, y))) in points.iter().zip(&coordinates).enumerate() {
        write!(body, r#"<circle class="mark" cx="{x:.1}" cy="{y:.1}" r="4"/><text x="{x:.1}" y="415" text-anchor="middle">{}</text>"#, if points.len() <= 10 || index % 2 == 0 { xml_escape(label) } else { String::new() }).expect("writing to a String cannot fail");
    }
    svg_shell(data, &body)
}

fn render_pie(data: &Value, points: &[(String, f64)]) -> String {
    let total = points.iter().map(|(_, value)| value.max(0.0)).sum::<f64>();
    if total <= 0.0 {
        return svg_shell(data, "");
    }
    let colors = [
        "#16a3a5", "#2563eb", "#f59e0b", "#ef4444", "#8b5cf6", "#64748b",
    ];
    let mut angle = -std::f64::consts::FRAC_PI_2;
    let mut body = String::new();
    for (index, (label, value)) in points.iter().enumerate() {
        let next = angle + value.max(0.0) / total * std::f64::consts::TAU;
        let (x1, y1) = (300.0 + 145.0 * angle.cos(), 235.0 + 145.0 * angle.sin());
        let (x2, y2) = (300.0 + 145.0 * next.cos(), 235.0 + 145.0 * next.sin());
        let large = i32::from(next - angle > std::f64::consts::PI);
        write!(body, r#"<path d="M 300 235 L {x1:.1} {y1:.1} A 145 145 0 {large} 1 {x2:.1} {y2:.1} Z" fill="{}"/><rect x="510" y="{}" width="14" height="14" fill="{}"/><text x="532" y="{}">{}</text>"#, colors[index % colors.len()], 90 + index * 28, colors[index % colors.len()], 102 + index * 28, xml_escape(label)).expect("writing to a String cannot fail");
        angle = next;
    }
    svg_shell(data, &body)
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn usize_as_f64(value: usize) -> f64 {
    f64::from(u32::try_from(value).unwrap_or(u32::MAX))
}

fn is_safe_svg(svg: &str) -> bool {
    let normalized = svg.to_ascii_lowercase();
    let active_fragments = [
        "<script",
        "<foreignobject",
        "javascript:",
        "data:text/html",
        "<!entity",
        "<!doctype",
    ];
    svg.trim_start().starts_with("<svg")
        && !active_fragments
            .iter()
            .any(|fragment| normalized.contains(fragment))
        && !has_svg_event_handler(&normalized)
        && !normalized.contains("href=\"http")
        && !normalized.contains("href='http")
}

fn has_svg_event_handler(svg: &str) -> bool {
    let bytes = svg.as_bytes();
    let mut index = 0;
    while index + 2 < bytes.len() {
        if bytes[index].is_ascii_whitespace()
            && bytes[index + 1] == b'o'
            && bytes[index + 2] == b'n'
        {
            let mut cursor = index + 3;
            while cursor < bytes.len()
                && (bytes[cursor].is_ascii_alphanumeric()
                    || matches!(bytes[cursor], b'-' | b'_' | b':'))
            {
                cursor += 1;
            }
            while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
                cursor += 1;
            }
            if cursor < bytes.len() && bytes[cursor] == b'=' {
                return true;
            }
        }
        index += 1;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::schema::v1::ContentBlock;
    use serde_json::json;

    #[test]
    fn normalizes_every_supported_alias() {
        assert_eq!(normalize_chart_type("dualAxes"), "dual-axes");
        assert_eq!(normalize_chart_type("heatmap"), "heat-map");
        assert_eq!(normalize_chart_type("organization"), "organization-chart");
        assert_eq!(supported_chart_types().len(), 24);
    }

    #[test]
    fn rejects_unknown_chart_types_without_losing_the_name() {
        assert_eq!(
            RichContent::chart("chart-1", json!({ "type": "magic", "data": [] })),
            Err(RichContentError::UnsupportedChartType("magic".into()))
        );
    }

    #[test]
    fn chart_preserves_source_data_and_builds_fallback_and_svg() {
        let chart = RichContent::chart(
            "message-1:chart-1",
            json!({
                "type": "column",
                "title": "Profit < R&D",
                "data": [
                    { "category": "FY2024", "value": 7.09 },
                    { "category": "FY2025", "value": 3.79 }
                ]
            }),
        )
        .unwrap();
        assert!(chart.fallback.starts_with("### Profit < R&D\n\n|"));
        assert!(chart.fallback.contains("FY2024"));
        assert!(!chart.fallback.contains("```vis-chart"));
        let svg = chart.svg.as_deref().unwrap();
        assert!(svg.contains("<svg"));
        assert!(svg.contains("Profit &lt; R&amp;D"));
        assert!(!svg.contains("<script"));
    }

    #[test]
    fn grouped_column_preview_preserves_categories_and_series_labels() {
        let chart = RichContent::chart(
            "chart-grouped",
            json!({
                "type": "column",
                "group": true,
                "data": [
                    { "category": "FY2024", "value": 7.09, "group": "Profit" },
                    { "category": "FY2025", "value": 3.79, "group": "Profit" },
                    { "category": "FY2024", "value": 4.54, "group": "R&D" },
                    { "category": "FY2025", "value": 6.41, "group": "R&D" }
                ]
            }),
        )
        .unwrap();
        assert!(chart.fallback.contains("FY2024"));
        assert!(chart.fallback.contains("Profit"));
        let svg = chart.svg.unwrap();
        assert_eq!(svg.matches("<rect x=").count(), 6);
        assert!(svg.contains("FY2024"));
        assert!(svg.contains("Profit"));
        assert!(svg.contains("R&amp;D"));
    }

    #[test]
    fn non_cartesian_chart_keeps_json_and_has_no_misleading_preview() {
        let chart = RichContent::chart(
            "chart-1",
            json!({ "type": "network", "data": [{ "source": "A", "target": "B" }] }),
        )
        .unwrap();
        assert_eq!(chart.data["type"], "network-graph");
        assert!(chart.svg.is_none());
        assert!(chart.fallback.contains("source"));
    }

    #[test]
    fn table_markdown_escapes_cells_and_keeps_rectangular_data() {
        let table = Table {
            columns: vec!["Year".into(), "Profit | USD".into()],
            rows: vec![vec!["FY2025".into(), "line 1\nline 2".into()]],
            title: Some("Results".into()),
        };
        let rich = RichContent::table("table-1", &table).unwrap();
        assert_eq!(rich.kind, RichContentKind::Table);
        assert!(rich.fallback.contains("Profit \\| USD"));
        assert!(rich.fallback.contains("line 1<br>line 2"));
    }

    #[test]
    fn malformed_table_is_rejected() {
        let error = RichContent::table(
            "table-1",
            &Table {
                columns: vec!["A".into(), "B".into()],
                rows: vec![vec!["only one".into()]],
                title: None,
            },
        )
        .unwrap_err();
        assert_eq!(
            error,
            RichContentError::InvalidTableRow {
                row: 0,
                expected: 2,
                actual: 1
            }
        );
    }

    #[test]
    fn acp_chunks_always_start_with_text_and_optionally_add_svg() {
        let chart = RichContent::chart(
            "chart-1",
            json!({ "type": "line", "data": [{ "time": "Q1", "value": 2 }] }),
        )
        .unwrap();
        let chunks = chart.to_acp_chunks();
        assert_eq!(chunks.len(), 2);
        let ContentBlock::Text(text) = &chunks[0].content else {
            panic!("expected text fallback");
        };
        assert!(text
            .meta
            .as_ref()
            .unwrap()
            .contains_key(RICH_CONTENT_NAMESPACE));
        let ContentBlock::Image(image) = &chunks[1].content else {
            panic!("expected SVG image");
        };
        assert_eq!(image.mime_type, "image/svg+xml");
        assert!(STANDARD.decode(&image.data).unwrap().starts_with(b"<svg"));
    }

    #[test]
    fn opaque_content_is_versioned_and_never_executes_itself() {
        let html = RichContent::opaque(
            "html-1",
            RichContentKind::Html,
            "text/html",
            json!({ "html": "<script>alert(1)</script>" }),
            "```html\n[Interactive content omitted]\n```",
        )
        .unwrap();
        assert_eq!(html.version, 1);
        assert!(html.svg.is_none());
        assert!(html
            .to_acp_chunks()
            .iter()
            .all(|chunk| matches!(chunk.content, ContentBlock::Text(_))));
    }

    #[test]
    fn widget_has_text_fallback_and_standard_resource_link() {
        let widget = RichContent::widget(
            "widget-1",
            "widget://quote/security/detail?symbol=TSLA.US&time_range=1",
            "[TSLA.US](https://longbridge.com/quote/tsla.us)",
        )
        .unwrap();
        let chunks = widget.to_acp_chunks();
        assert_eq!(chunks.len(), 2);
        assert!(matches!(chunks[0].content, ContentBlock::Text(_)));
        let ContentBlock::ResourceLink(resource) = &chunks[1].content else {
            panic!("expected widget resource link");
        };
        assert_eq!(
            resource.uri,
            "widget://quote/security/detail?symbol=TSLA.US&time_range=1"
        );
    }

    #[test]
    fn widget_rejects_non_widget_and_markup_injection_uris() {
        assert_eq!(
            RichContent::widget("widget-1", "https://example.com", "fallback"),
            Err(RichContentError::InvalidWidgetUri)
        );
        assert_eq!(
            RichContent::widget("widget-1", "widget://quote/<script>", "fallback"),
            Err(RichContentError::InvalidWidgetUri)
        );
    }

    #[test]
    fn extracts_multiple_complete_charts_and_ignores_partial_fences() {
        let markdown = concat!(
            "Before\n```vis-chart\n{\"type\":\"pie\",\"data\":[{\"category\":\"A\",\"value\":1}]}\n```\n",
            "Between\n```vis-chart\n{\"type\":\"line\",\"data\":[{\"time\":\"Q1\",\"value\":2}]}\n```\n",
            "```vis-chart\n{\"type\":\"column\""
        );
        let charts = charts_from_markdown(markdown, "message-1");
        assert_eq!(charts.len(), 2);
        assert_eq!(charts[0].content_id, "message-1:chart-1");
        assert_eq!(charts[1].data["type"], "line");
    }

    #[test]
    fn accepts_passive_svg_and_rejects_active_content() {
        let svg = RichContent::svg(
            "svg-1",
            r#"<svg xmlns="http://www.w3.org/2000/svg"><circle cx="5" cy="5" r="4"/></svg>"#,
            "Circle",
        )
        .unwrap();
        assert_eq!(svg.kind, RichContentKind::Svg);
        assert!(svg.svg_preview_chunk().is_some());

        assert_eq!(
            RichContent::svg(
                "svg-2",
                r#"<svg xmlns="http://www.w3.org/2000/svg"><script>alert(1)</script></svg>"#,
                "Unsafe",
            ),
            Err(RichContentError::UnsafeSvg)
        );
        assert_eq!(
            RichContent::svg(
                "svg-3",
                r#"<svg xmlns="http://www.w3.org/2000/svg"><image href="https://example.com/a.png"/></svg>"#,
                "External",
            ),
            Err(RichContentError::UnsafeSvg)
        );
        assert_eq!(
            RichContent::svg(
                "svg-4",
                r#"<svg xmlns="http://www.w3.org/2000/svg" onload = "alert(1)"/>"#,
                "Handler",
            ),
            Err(RichContentError::UnsafeSvg)
        );
    }
}
