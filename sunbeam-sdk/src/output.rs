//! CLI output formatting (table, JSON, YAML).

use crate::error::{Result, SunbeamError};
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
            // Fallback: pretty JSON when no table renderer is provided
            println!("{}", serde_json::to_string_pretty(val)?);
        }
    }
    Ok(())
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
pub fn read_json_input(flag: Option<&str>) -> Result<serde_json::Value> {
    let raw = match flag {
        Some("-") | None => {
            let mut buf = String::new();
            std::io::Read::read_to_string(&mut std::io::stdin(), &mut buf)?;
            buf
        }
        Some(v) => v.to_string(),
    };
    serde_json::from_str(&raw).map_err(|e| SunbeamError::Other(format!("invalid JSON input: {e}")))
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
