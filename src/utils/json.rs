//! Generic JSON helpers: null stripping and flattening arbitrary documents
//! into tables for export.

use serde_json::{Map, Value};

/// Recursively remove object entries whose value is `null`, and drop `null`
/// elements from arrays.
///
/// Older statement files serialize empty sections as `null` instead of `[]`,
/// which `#[serde(default)]` does not tolerate (it only fills *missing* keys).
/// Stripping nulls first makes those files deserialize like current ones.
pub fn strip_nulls(value: &mut Value) {
    match value {
        Value::Object(map) => {
            map.retain(|_, v| !v.is_null());
            for v in map.values_mut() {
                strip_nulls(v);
            }
        }
        Value::Array(items) => {
            items.retain(|v| !v.is_null());
            for v in items {
                strip_nulls(v);
            }
        }
        _ => {}
    }
}

/// A table derived from one node of a JSON document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlatTable {
    /// File-name-friendly identifier, e.g. `stock_holding_details_stock_holding`.
    pub name: String,
    /// Human-readable title, e.g. `StockHoldingDetails / StockHolding`.
    pub title: String,
    pub headers: Vec<String>,
    pub rows: Vec<Vec<String>>,
}

/// Flatten an arbitrary JSON document into tables, one per object or array
/// node, so a document with an unknown schema can still be exported.
///
/// Rules:
/// - Scalar fields of an object become one row; nested objects are flattened
///   into dotted columns (`Fee.Amount`).
/// - An array of objects becomes one table; array-valued fields of its
///   elements become sub-tables whose rows carry the parent's scalar fields
///   as leading context columns (e.g. `Market` on each holding row).
/// - Tables with no rows are omitted.
pub fn flatten_tables(value: &Value) -> Vec<FlatTable> {
    let mut out = Vec::new();
    walk(&mut out, &[], value, &[]);
    out
}

type Context = Vec<(String, String)>;

fn walk(out: &mut Vec<FlatTable>, path: &[String], value: &Value, context: &[(String, String)]) {
    match value {
        Value::Object(map) => walk_object(out, path, map, context),
        Value::Array(items) => walk_array(out, path, items, context),
        _ => {}
    }
}

fn walk_object(
    out: &mut Vec<FlatTable>,
    path: &[String],
    map: &Map<String, Value>,
    context: &[(String, String)],
) {
    let scalars = scalar_columns(map, "");
    if !scalars.is_empty() {
        let (headers, row) = with_context(context, scalars);
        push_table(out, path, headers, vec![row]);
    }
    for (key, child) in map {
        if child.is_object() && !is_flat_object(child) || child.is_array() {
            let child_path = join_path(path, key);
            walk(out, &child_path, child, context);
        }
    }
}

fn walk_array(
    out: &mut Vec<FlatTable>,
    path: &[String],
    items: &[Value],
    context: &[(String, String)],
) {
    let objects: Vec<&Map<String, Value>> = items.iter().filter_map(Value::as_object).collect();
    if objects.is_empty() {
        let rows: Vec<Vec<String>> = items
            .iter()
            .filter(|v| !v.is_object() && !v.is_array())
            .map(|v| {
                let (_, row) = with_context(context, vec![("Value".to_string(), scalar_text(v))]);
                row
            })
            .collect();
        let (headers, _) = with_context(context, vec![("Value".to_string(), String::new())]);
        push_table(out, path, headers, rows);
        return;
    }

    // Union of scalar column names across elements, in first-seen order.
    let mut columns: Vec<String> = Vec::new();
    let per_row: Vec<Vec<(String, String)>> = objects
        .iter()
        .map(|obj| {
            let cols = scalar_columns(obj, "");
            for (k, _) in &cols {
                if !columns.contains(k) {
                    columns.push(k.clone());
                }
            }
            cols
        })
        .collect();

    let mut headers: Vec<String> = context.iter().map(|(k, _)| k.clone()).collect();
    headers.extend(columns.iter().cloned());
    let rows: Vec<Vec<String>> = per_row
        .iter()
        .map(|cols| {
            let mut row: Vec<String> = context.iter().map(|(_, v)| v.clone()).collect();
            for col in &columns {
                row.push(
                    cols.iter()
                        .find(|(k, _)| k == col)
                        .map(|(_, v)| v.clone())
                        .unwrap_or_default(),
                );
            }
            row
        })
        .collect();
    push_table(out, path, headers, rows);

    // Array-valued fields inside elements become sub-tables with the element's
    // scalars as context, merged across all elements.
    let mut sub_keys: Vec<String> = Vec::new();
    for obj in &objects {
        for (k, v) in *obj {
            if v.is_array() && !sub_keys.contains(k) {
                sub_keys.push(k.clone());
            }
        }
    }
    for key in sub_keys {
        let sub_path = join_path(path, &key);
        let mut merged = FlatTableBuilder::default();
        for (obj, cols) in objects.iter().zip(&per_row) {
            let Some(Value::Array(children)) = obj.get(&key) else {
                continue;
            };
            let mut ctx: Context = context.to_vec();
            ctx.extend(cols.iter().cloned());
            let mut tmp = Vec::new();
            walk_array(&mut tmp, &sub_path, children, &ctx);
            for t in tmp {
                merged.absorb(t);
            }
        }
        out.extend(merged.finish());
    }
}

/// Accumulates tables produced per element into one table per name, so a
/// sub-array spread across many parent elements yields a single table.
#[derive(Default)]
struct FlatTableBuilder {
    tables: Vec<FlatTable>,
}

impl FlatTableBuilder {
    fn absorb(&mut self, table: FlatTable) {
        if let Some(existing) = self.tables.iter_mut().find(|t| t.name == table.name) {
            if existing.headers == table.headers {
                existing.rows.extend(table.rows);
            } else {
                // Column sets differ between parents: re-align rows by header name.
                for h in &table.headers {
                    if !existing.headers.contains(h) {
                        existing.headers.push(h.clone());
                        for row in &mut existing.rows {
                            row.push(String::new());
                        }
                    }
                }
                for row in table.rows {
                    let mut aligned = vec![String::new(); existing.headers.len()];
                    for (h, v) in table.headers.iter().zip(row) {
                        if let Some(i) = existing.headers.iter().position(|x| x == h) {
                            aligned[i] = v;
                        }
                    }
                    existing.rows.push(aligned);
                }
            }
        } else {
            self.tables.push(table);
        }
    }

    fn finish(self) -> Vec<FlatTable> {
        self.tables
    }
}

fn with_context(
    context: &[(String, String)],
    cols: Vec<(String, String)>,
) -> (Vec<String>, Vec<String>) {
    let mut headers: Vec<String> = context.iter().map(|(k, _)| k.clone()).collect();
    let mut row: Vec<String> = context.iter().map(|(_, v)| v.clone()).collect();
    for (k, v) in cols {
        headers.push(k);
        row.push(v);
    }
    (headers, row)
}

/// Scalar fields of `map`, with nested flat objects flattened into dotted keys.
fn scalar_columns(map: &Map<String, Value>, prefix: &str) -> Vec<(String, String)> {
    let mut cols = Vec::new();
    for (k, v) in map {
        let name = if prefix.is_empty() {
            k.clone()
        } else {
            format!("{prefix}.{k}")
        };
        match v {
            Value::Object(inner) if is_flat_object(v) => cols.extend(scalar_columns(inner, &name)),
            Value::Object(_) | Value::Array(_) | Value::Null => {}
            other => cols.push((name, scalar_text(other))),
        }
    }
    cols
}

/// An object whose values are all scalars (or nested flat objects) is
/// flattened into its parent's row rather than becoming its own table.
fn is_flat_object(value: &Value) -> bool {
    match value {
        Value::Object(map) => map.values().all(|v| match v {
            Value::Array(_) => false,
            Value::Object(_) => is_flat_object(v),
            _ => true,
        }),
        _ => false,
    }
}

fn scalar_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

fn join_path(path: &[String], key: &str) -> Vec<String> {
    let mut p = path.to_vec();
    p.push(key.to_string());
    p
}

fn push_table(
    out: &mut Vec<FlatTable>,
    path: &[String],
    headers: Vec<String>,
    rows: Vec<Vec<String>>,
) {
    if rows.is_empty() {
        return;
    }
    let name = if path.is_empty() {
        "document".to_string()
    } else {
        path.iter()
            .map(|s| to_snake_case(s))
            .collect::<Vec<_>>()
            .join("_")
    };
    let title = if path.is_empty() {
        "Document".to_string()
    } else {
        path.join(" / ")
    };
    out.push(FlatTable {
        name,
        title,
        headers,
        rows,
    });
}

/// `StockHoldingDetails` -> `stock_holding_details`.
pub fn to_snake_case(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 4);
    let chars: Vec<char> = s.chars().collect();
    for (i, c) in chars.iter().enumerate() {
        if c.is_uppercase() {
            let prev_lower =
                i > 0 && (chars[i - 1].is_lowercase() || chars[i - 1].is_ascii_digit());
            let next_lower = chars.get(i + 1).is_some_and(|n| n.is_lowercase());
            if i > 0 && (prev_lower || next_lower) && !out.ends_with('_') {
                out.push('_');
            }
            out.extend(c.to_lowercase());
        } else if c.is_alphanumeric() {
            out.push(*c);
        } else if !out.ends_with('_') {
            out.push('_');
        }
    }
    out.trim_matches('_').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn strip_nulls_removes_null_entries_recursively() {
        let mut v = json!({"a": null, "b": {"c": null, "d": 1}, "e": [null, {"f": null}]});
        strip_nulls(&mut v);
        assert_eq!(v, json!({"b": {"d": 1}, "e": [{}]}));
    }

    #[test]
    fn snake_case_handles_acronyms_and_digits() {
        assert_eq!(
            to_snake_case("StockHoldingDetails"),
            "stock_holding_details"
        );
        assert_eq!(to_snake_case("Fee0Name"), "fee0_name");
        assert_eq!(to_snake_case("IPOSub"), "ipo_sub");
    }

    #[test]
    fn flattens_legacy_statement_shape() {
        let doc = json!({
            "Date": "2021.11.30",
            "AssetDetail": {"Cash": "1", "Total": "2"},
            "CashDetails": {
                "Currency": "HKD",
                "CashDetail": [{"Currency": "HKD", "Amount": "1"}, {"Currency": "USD", "Amount": "2"}]
            },
            "StockHoldingDetails": [
                {"Market": "US", "StockHolding": [{"StName": "DAL", "Total": "199"}]},
                {"Market": "HK", "StockHolding": [{"StName": "00175", "Total": "1000"}]}
            ],
            "Empty": null,
            "Nothing": []
        });
        let tables = flatten_tables(&doc);
        let names: Vec<&str> = tables.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "document",
                "cash_details",
                "cash_details_cash_detail",
                "stock_holding_details",
                "stock_holding_details_stock_holding",
            ]
        );

        let doc_table = &tables[0];
        assert_eq!(
            doc_table.headers,
            ["Date", "AssetDetail.Cash", "AssetDetail.Total"]
        );
        assert_eq!(doc_table.rows, [["2021.11.30", "1", "2"]]);

        let holdings = &tables[4];
        assert_eq!(holdings.title, "StockHoldingDetails / StockHolding");
        assert_eq!(holdings.headers, ["Market", "StName", "Total"]);
        assert_eq!(
            holdings.rows,
            [["US", "DAL", "199"], ["HK", "00175", "1000"]]
        );

        let cash = &tables[2];
        assert_eq!(cash.headers, ["Currency", "Amount"]);
        assert_eq!(cash.rows.len(), 2);
    }
}
