//! CLI output formatting (table, JSON, YAML).

use sdk::error::Result;
use serde::Serialize;

// ---------------------------------------------------------------------------
// OutputFormat
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Default, clap::ValueEnum)]
/// Outputformat.
pub enum OutputFormat {
    #[default]
    /// Table.
    Table,
    /// Json.
    Json,
    /// Yaml.
    Yaml,
}

/// Render a single serialisable value in the requested format.
pub fn render<T: Serialize>(val: &T, format: OutputFormat) -> Result<()> {
    match format {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(val)?);
        }
        OutputFormat::Yaml => {
            print!("{}", serde_yaml::to_string(val)?);
        }
        OutputFormat::Table => {
            let value = serde_json::to_value(val)?;
            match &value {
                serde_json::Value::Object(_) => println!("{}", detail(&value)),
                // Scalars and arrays have no field names to tabulate.
                _ => println!("{}", serde_json::to_string_pretty(&value)?),
            }
        }
    }
    Ok(())
}

/// Render an object as a two-column FIELD/VALUE table for detail views.
///
/// Scalars print plainly; flat arrays join with ", "; nested objects and
/// arrays of objects fall back to compact JSON; newlines in strings are
/// flattened so rows stay one line tall. Non-object values render as pretty
/// JSON.
pub fn detail(value: &serde_json::Value) -> String {
    use serde_json::Value;
    let map = match value {
        Value::Object(map) => map,
        other => {
            return serde_json::to_string_pretty(other).unwrap_or_else(|_| "?".into());
        }
    };
    let rows: Vec<Vec<String>> = map
        .iter()
        .map(|(k, v)| vec![k.clone(), detail_cell(v)])
        .collect();
    table(&rows, &["FIELD", "VALUE"])
}

/// Truncate `s` to `max` chars, appending an ellipsis when cut.
pub(crate) fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push('…');
    out
}

/// One-line cell rendering for [`detail`]. Long values are truncated so a
/// single field can't blow out the table width; the full value is available
/// via `-o json` / `-o yaml`.
fn detail_cell(v: &serde_json::Value) -> String {
    use serde_json::Value;
    let rendered = match v {
        Value::Null => "-".into(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => s.replace('\n', " ⏎ "),
        Value::Array(items) => {
            if items.is_empty() {
                "-".into()
            } else if items.iter().all(|i| !i.is_object() && !i.is_array()) {
                items.iter().map(detail_cell).collect::<Vec<_>>().join(", ")
            } else {
                serde_json::to_string(v).unwrap_or_else(|_| "?".into())
            }
        }
        Value::Object(_) => serde_json::to_string(v).unwrap_or_else(|_| "?".into()),
    };
    truncate(&rendered, 120)
}

/// Render a list of items as table / json / yaml.
///
/// `to_row` converts each item into a `Vec<String>` of column values matching
/// the order of `headers`.
pub fn render_list<T: Serialize>(
    rows: &[T],
    headers: &[&str],
    to_row: fn(&T) -> Vec<String>,
    format: OutputFormat,
) -> Result<()> {
    match format {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(rows)?);
        }
        OutputFormat::Yaml => {
            print!("{}", serde_yaml::to_string(rows)?);
        }
        OutputFormat::Table => {
            let table_rows: Vec<Vec<String>> = rows.iter().map(to_row).collect();
            println!("{}", table(table_rows.as_slice(), headers));
        }
    }
    Ok(())
}

/// Read JSON input from a `--data` flag value or stdin when the value is `"-"`.
#[cfg(test)]
pub fn read_json_input(flag: Option<&str>) -> Result<serde_json::Value> {
    let raw = match flag {
        Some("-") | None => {
            let mut buf = String::new();
            std::io::Read::read_to_string(&mut std::io::stdin(), &mut buf)?;
            buf
        }
        Some(v) => v.to_string(),
    };
    serde_json::from_str(&raw)
        .map_err(|e| sdk::error::SunbeamError::Other(format!("invalid JSON input: {e}")))
}

/// Return an aligned text table. Columns padded to max width.
pub fn table(rows: &[Vec<String>], headers: &[&str]) -> String {
    if headers.is_empty() {
        return String::new();
    }

    let mut col_widths: Vec<usize> = headers.iter().map(|h| h.len()).collect();
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            if i < col_widths.len() {
                col_widths[i] = col_widths[i].max(cell.len());
            }
        }
    }

    let header_line: String = headers
        .iter()
        .enumerate()
        .map(|(i, h)| format!("{:<width$}", h, width = col_widths[i]))
        .collect::<Vec<_>>()
        .join("  ");

    let separator: String = col_widths
        .iter()
        .map(|&w| "-".repeat(w))
        .collect::<Vec<_>>()
        .join("  ");

    let mut lines = vec![header_line, separator];

    for row in rows {
        let cells: Vec<String> = (0..headers.len())
            .map(|i| {
                let val = row.get(i).map(|s| s.as_str()).unwrap_or("");
                format!("{:<width$}", val, width = col_widths[i])
            })
            .collect();
        lines.push(cells.join("  "));
    }

    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_table_basic() {
        let rows = vec![
            vec!["abc".to_string(), "def".to_string()],
            vec!["x".to_string(), "longer".to_string()],
        ];
        let result = table(&rows, &["Col1", "Col2"]);
        assert!(result.contains("Col1"));
        assert!(result.contains("Col2"));
        assert!(result.contains("abc"));
        assert!(result.contains("longer"));
    }

    #[test]
    fn test_table_empty_headers() {
        let result = table(&[], &[]);
        assert!(result.is_empty());
    }

    #[test]
    fn test_table_column_widths() {
        let rows = vec![vec!["short".to_string(), "x".to_string()]];
        let result = table(&rows, &["LongHeader", "H2"]);
        for line in result.lines().skip(2) {
            assert!(line.starts_with("short     "));
        }
    }

    #[test]
    fn test_render_json() {
        let val = serde_json::json!({"key": "value"});
        // Just ensure it doesn't panic
        render(&val, OutputFormat::Json).unwrap();
    }

    #[test]
    fn test_detail_renders_field_value_table() {
        let val = serde_json::json!({
            "id": "01K",
            "title": "Fix crash",
            "priority": "high",
            "blocked": false,
            "completed_at": "",
            "labels": ["bug", "backend"],
            "empty": [],
            "assignees": [{"subject": "user:1"}],
            "meta": {"a": 1},
            "missing": null,
            "description": "line one\nline two",
        });
        let out = detail(&val);
        let w = "completed_at".len(); // longest key drives the FIELD width
        let row = |k: &str, v: &str| format!("{k:<w$}  {v}");
        assert!(
            out.lines()
                .next()
                .unwrap()
                .starts_with(&row("FIELD", "VALUE")),
            "{out}"
        );
        assert!(out.contains(&row("id", "01K")), "{out}");
        assert!(out.contains(&row("blocked", "false")), "{out}");
        assert!(out.contains(&row("labels", "bug, backend")), "{out}");
        assert!(out.contains(&row("empty", "-")), "{out}");
        assert!(out.contains(&row("missing", "-")), "{out}");
        assert!(
            out.contains(&row("assignees", "[{\"subject\":\"user:1\"}]")),
            "{out}"
        );
        assert!(out.contains(&row("meta", "{\"a\":1}")), "{out}");
        assert!(
            out.contains(&row("description", "line one ⏎ line two")),
            "{out}"
        );
        // No raw JSON braces at line starts — this is a table, not a dump.
        assert!(!out.starts_with('{'), "{out}");
    }

    #[test]
    fn test_detail_non_object_falls_back_to_json() {
        let val = serde_json::json!(["a", "b"]);
        let out = detail(&val);
        assert!(out.starts_with('['), "{out}");
    }

    #[test]
    fn test_render_yaml() {
        let val = serde_json::json!({"key": "value"});
        render(&val, OutputFormat::Yaml).unwrap();
    }

    #[test]
    fn test_render_list_table() {
        #[derive(Serialize)]
        struct Item {
            name: String,
        }
        let items = vec![Item {
            name: "test".into(),
        }];
        render_list(
            &items,
            &["NAME"],
            |i| vec![i.name.clone()],
            OutputFormat::Table,
        )
        .unwrap();
    }

    #[test]
    fn test_read_json_input_inline() {
        let val = read_json_input(Some(r#"{"a":1}"#)).unwrap();
        assert_eq!(val["a"], 1);
    }
}
