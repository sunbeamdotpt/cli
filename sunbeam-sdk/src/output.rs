/// Print a step header.
pub fn step(msg: &str) {
    println!("\n==> {msg}");
}

/// Print a success/info line.
pub fn ok(msg: &str) {
    println!("    {msg}");
}

/// Print a warning to stderr.
pub fn warn(msg: &str) {
    eprintln!("    WARN: {msg}");
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
        // Header should set minimum width
        for line in result.lines().skip(2) {
            // Data row: "short" should be padded to "LongHeader" width
            assert!(line.starts_with("short     "));
        }
    }
}
