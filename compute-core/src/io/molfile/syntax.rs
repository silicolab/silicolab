//! The V3000 line grammar shared by the reader and writer: `M  V30 ` records,
//! `-` continuations, and space-separated fields with quoted strings and
//! parenthesized lists.

use anyhow::{Context, Result, anyhow, bail};

const V30_PREFIX: &str = "M  V30 ";
/// The format's maximum line length, including the prefix and any `-`.
const MAX_LINE: usize = 80;

/// One logical V3000 record with its continuations joined.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Record {
    /// 1-based line in the source where the record starts.
    pub line: usize,
    pub text: String,
}

/// The file's V3000 records up to `M  END`, and the lines after it.
pub(super) struct Records<'a> {
    pub records: Vec<Record>,
    pub trailing: Vec<&'a str>,
    pub ignored_lines: usize,
}

/// Join the `M  V30` records starting at 0-based line `start`.
pub(super) fn collect_records<'a>(lines: &[&'a str], start: usize) -> Result<Records<'a>> {
    let mut records = Vec::new();
    let mut ignored_lines = 0;
    let mut pending: Option<Record> = None;
    for (index, raw) in lines.iter().enumerate().skip(start) {
        let line = raw.trim_end();
        if line == "M  END" {
            if pending.is_some() {
                bail!("line {}: `M  END` after a continuation", index + 1);
            }
            return Ok(Records {
                records,
                trailing: lines[index + 1..].to_vec(),
                ignored_lines,
            });
        }
        let content = if line == "M  V30" {
            Some("")
        } else {
            line.strip_prefix(V30_PREFIX)
        };
        let Some(content) = content else {
            if pending.is_some() {
                bail!("line {}: expected a continuation record", index + 1);
            }
            ignored_lines += 1;
            continue;
        };
        let (content, continues) = match content.strip_suffix('-') {
            Some(head) => (head, true),
            None => (content, false),
        };
        let mut record = pending.take().unwrap_or(Record {
            line: index + 1,
            text: String::new(),
        });
        record.text.push_str(content);
        if continues {
            pending = Some(record);
        } else {
            records.push(record);
        }
    }
    bail!("molfile ended without `M  END`")
}

/// Split a record into fields. Spaces inside `"…"` and `(…)` do not separate.
pub(super) fn split_fields(text: &str) -> Result<Vec<String>> {
    let mut fields = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    let mut depth = 0usize;
    for c in text.chars() {
        match c {
            '"' => in_quotes = !in_quotes,
            '(' if !in_quotes => depth += 1,
            ')' if !in_quotes => {
                depth = depth
                    .checked_sub(1)
                    .ok_or_else(|| anyhow!("unbalanced `)`"))?;
            }
            c if c.is_whitespace() && !in_quotes && depth == 0 => {
                if !current.is_empty() {
                    fields.push(std::mem::take(&mut current));
                }
                continue;
            }
            _ => {}
        }
        current.push(c);
    }
    if in_quotes {
        bail!("unterminated quoted string");
    }
    if depth != 0 {
        bail!("unbalanced `(`");
    }
    if !current.is_empty() {
        fields.push(current);
    }
    Ok(fields)
}

/// Remove the quotes of a `"…"` value, where `""` stands for one quote.
pub(super) fn unquote(value: &str) -> String {
    value
        .strip_prefix('"')
        .and_then(|inner| inner.strip_suffix('"'))
        .map_or_else(|| value.to_string(), |inner| inner.replace("\"\"", "\""))
}

/// Quote a value when it could not otherwise stand as one field.
pub(super) fn quote(value: &str) -> String {
    let plain = !value.is_empty()
        && !value
            .chars()
            .any(|c| c.is_whitespace() || matches!(c, '"' | '(' | ')' | '='));
    if plain {
        value.to_string()
    } else {
        format!("\"{}\"", value.replace('"', "\"\""))
    }
}

/// Split a `KEY=value` field. The key comes back upper-cased.
pub(super) fn key_value(field: &str) -> Result<(String, &str)> {
    let (key, value) = field
        .split_once('=')
        .ok_or_else(|| anyhow!("expected KEY=value, found `{field}`"))?;
    Ok((key.to_ascii_uppercase(), value))
}

/// Parse a `(count item item …)` list, checking the count.
pub(super) fn parse_list(value: &str) -> Result<Vec<&str>> {
    let inner = value
        .strip_prefix('(')
        .and_then(|inner| inner.strip_suffix(')'))
        .ok_or_else(|| anyhow!("expected a parenthesized list, found `{value}`"))?;
    let mut items = inner.split_whitespace();
    let count = items
        .next()
        .ok_or_else(|| anyhow!("empty list"))?
        .parse::<usize>()
        .context("list count is not a number")?;
    let items: Vec<&str> = items.collect();
    if items.len() != count {
        bail!("list declares {count} item(s) but has {}", items.len());
    }
    Ok(items)
}

/// Append a record, wrapping it with `-` continuations at the line limit.
pub(super) fn push_record(output: &mut String, record: &str) {
    let width = MAX_LINE - V30_PREFIX.len() - 1;
    let chars: Vec<char> = record.chars().collect();
    let mut chunks = chars.chunks(width).peekable();
    if chunks.peek().is_none() {
        output.push_str("M  V30\n");
    }
    while let Some(chunk) = chunks.next() {
        output.push_str(V30_PREFIX);
        output.extend(chunk);
        if chunks.peek().is_some() {
            output.push('-');
        }
        output.push('\n');
    }
}
