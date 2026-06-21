//! Parser for GROMACS `.xvg` (Grace/xmgrace) data files.
//!
//! Every GROMACS analysis tool (`gmx energy`, `rms`, `gyrate`, `mindist`, ...)
//! writes its output as an `.xvg` file: a column of an independent variable
//! (usually time or step) followed by one or more data series, preceded by
//! `#` comments and `@` Grace metadata directives. This is the single parser
//! that turns any of those files into numeric series for plotting or further
//! analysis — it knows nothing about which tool produced the file.

use anyhow::{Result, anyhow};

/// A parsed `.xvg` data set: metadata plus column-major numeric data.
#[derive(Debug, Clone, Default)]
pub struct Xvg {
    /// Plot title (`@ title "..."`), if present.
    pub title: Option<String>,
    /// X-axis label (`@ xaxis label "..."`), if present.
    pub x_label: Option<String>,
    /// Y-series legends (`@ sN legend "..."`), in order.
    pub y_labels: Vec<String>,
    /// Column-major numeric data: `columns[c][r]` is row `r` of column `c`.
    /// Column 0 is the independent variable; columns `1..` are the data series.
    pub columns: Vec<Vec<f64>>,
}

impl Xvg {
    /// Number of data rows.
    pub fn rows(&self) -> usize {
        self.columns.first().map(|c| c.len()).unwrap_or(0)
    }

    /// Borrow one column's values, or `None` if the index is out of range.
    pub fn series(&self, column: usize) -> Option<&[f64]> {
        self.columns.get(column).map(Vec::as_slice)
    }

    /// Arithmetic mean of a column, or `None` if the column is missing/empty.
    pub fn mean(&self, column: usize) -> Option<f64> {
        let values = self.series(column)?;
        if values.is_empty() {
            return None;
        }
        Some(values.iter().sum::<f64>() / values.len() as f64)
    }

    /// Last value of a column, or `None` if the column is missing/empty.
    pub fn final_value(&self, column: usize) -> Option<f64> {
        self.series(column)?.last().copied()
    }
}

/// Parse the contents of an `.xvg` file.
///
/// Lines beginning with `#` are comments and ignored. Lines beginning with `@`
/// are Grace directives; `title`, `xaxis label`, and `sN legend` are extracted
/// and the rest ignored. All remaining non-blank lines are whitespace-separated
/// numeric rows. The column count is fixed by the first data row; rows with
/// fewer columns are an error, extra columns are ignored.
pub fn parse_xvg(text: &str) -> Result<Xvg> {
    let mut xvg = Xvg::default();

    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(directive) = line.strip_prefix('@') {
            apply_directive(&mut xvg, directive.trim());
            continue;
        }

        let values: Vec<f64> = line
            .split_whitespace()
            .map(|tok| {
                tok.parse::<f64>()
                    .map_err(|_| anyhow!("invalid numeric value '{tok}' in .xvg data row"))
            })
            .collect::<Result<_>>()?;

        if values.is_empty() {
            continue;
        }

        if xvg.columns.is_empty() {
            xvg.columns = vec![Vec::new(); values.len()];
        } else if values.len() < xvg.columns.len() {
            return Err(anyhow!(
                "ragged .xvg row: expected {} columns, found {}",
                xvg.columns.len(),
                values.len()
            ));
        }

        for (column, value) in xvg.columns.iter_mut().zip(values) {
            column.push(value);
        }
    }

    Ok(xvg)
}

/// Apply a single `@` Grace directive (the `@` already stripped).
fn apply_directive(xvg: &mut Xvg, directive: &str) {
    if let Some(rest) = directive.strip_prefix("title") {
        if let Some(text) = first_quoted(rest) {
            xvg.title = Some(text);
        }
    } else if let Some(rest) = directive.strip_prefix("xaxis") {
        if let Some(label) = rest.trim().strip_prefix("label")
            && let Some(text) = first_quoted(label)
        {
            xvg.x_label = Some(text);
        }
    } else if directive.starts_with('s') {
        // `s0 legend "..."`, `s1 legend "..."`, ...
        if let Some(idx) = directive.find("legend")
            && let Some(text) = first_quoted(&directive[idx + "legend".len()..])
        {
            xvg.y_labels.push(text);
        }
    }
}

/// Extract the first double-quoted substring, if any.
fn first_quoted(text: &str) -> Option<String> {
    let start = text.find('"')? + 1;
    let end = text[start..].find('"')? + start;
    Some(text[start..end].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
# This file was created by gmx energy
@    title \"GROMACS Energies\"
@    xaxis  label \"Time (ps)\"
@    yaxis  label \"(kJ/mol)\"
@ s0 legend \"Potential\"
0.000000   -1234.5
1.000000   -1240.0
2.000000   -1250.5
";

    #[test]
    fn parses_two_column_xvg_with_legends() {
        let xvg = parse_xvg(SAMPLE).expect("parsed");
        assert_eq!(xvg.title.as_deref(), Some("GROMACS Energies"));
        assert_eq!(xvg.x_label.as_deref(), Some("Time (ps)"));
        assert_eq!(xvg.y_labels, vec!["Potential".to_string()]);
        assert_eq!(xvg.columns.len(), 2);
        assert_eq!(xvg.rows(), 3);
        assert_eq!(xvg.series(0), Some([0.0, 1.0, 2.0].as_slice()));
    }

    #[test]
    fn xvg_mean_and_final_value() {
        let xvg = parse_xvg(SAMPLE).expect("parsed");
        let mean = xvg.mean(1).expect("mean");
        assert!((mean - (-1241.6666)).abs() < 1e-2, "mean was {mean}");
        assert_eq!(xvg.final_value(1), Some(-1250.5));
    }

    #[test]
    fn ragged_row_is_an_error() {
        let bad = "0.0 1.0\n1.0\n";
        assert!(parse_xvg(bad).is_err());
    }

    #[test]
    fn blank_and_comment_only_file_parses_empty() {
        let xvg = parse_xvg("# only a comment\n\n@ title \"x\"\n").expect("parsed");
        assert_eq!(xvg.rows(), 0);
        assert!(xvg.series(0).is_none());
        assert_eq!(xvg.title.as_deref(), Some("x"));
    }
}
