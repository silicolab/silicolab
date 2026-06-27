use anyhow::{Context, Result, anyhow, bail};

use crate::domain::chemistry::normalized_symbol;

pub(super) fn element_from_atom_name(atom_name: &str) -> String {
    let letters = atom_name
        .chars()
        .filter(|ch| ch.is_ascii_alphabetic())
        .collect::<String>();
    if letters.is_empty() {
        return "X".to_string();
    }

    if letters.len() >= 2 {
        let two_letter = normalized_symbol(&letters[..2]);
        if !two_letter.is_empty() {
            return two_letter;
        }
    }

    let one_letter = normalized_symbol(&letters[..1]);
    if one_letter.is_empty() {
        "X".to_string()
    } else {
        one_letter
    }
}

pub(super) fn infer_residue_fields<'a>(
    line: &'a str,
    fields: &'a [&'a str],
    coordinate_start: usize,
    line_number: usize,
) -> Result<(char, &'a str)> {
    let metadata = &fields[2..coordinate_start];
    match metadata {
        [_, _, chain_or_residue] => split_chain_and_residue(chain_or_residue)
            .or(Some((' ', *chain_or_residue)))
            .ok_or_else(|| anyhow!("invalid chain/residue token on PDB atom line {line_number}")),
        [_, _, chain_id, residue_seq] => Ok((chain_id.chars().next().unwrap_or(' '), *residue_seq)),
        _ => {
            let chain_id = field(line, 21, 22).chars().next().unwrap_or(' ');
            let residue_seq = field(line, 22, 26).trim();
            if residue_seq.is_empty() {
                bail!("missing residue sequence on PDB atom line {line_number}");
            }
            Ok((chain_id, residue_seq))
        }
    }
}

fn split_chain_and_residue(token: &str) -> Option<(char, &str)> {
    let mut chars = token.char_indices();
    let (_, chain_id) = chars.next()?;
    if !chain_id.is_ascii_alphabetic() {
        return None;
    }

    let residue_start = chars.next().map(|(index, _)| index).unwrap_or(token.len());
    let residue_seq = token[residue_start..].trim();
    if residue_seq.is_empty()
        || !residue_seq
            .chars()
            .all(|ch| ch == '-' || ch.is_ascii_digit())
    {
        return None;
    }

    Some((chain_id, residue_seq))
}

pub(super) fn find_coordinate_start(fields: &[&str]) -> Option<usize> {
    fields.windows(3).position(|window| {
        window.iter().all(|value| {
            value.parse::<f64>().is_ok()
                && (value.contains('.') || value.contains('e') || value.contains('E'))
        })
    })
}

pub(super) fn parse_fixed_width_usize_or_fallback(
    line: &str,
    start: usize,
    end: usize,
    fallback: Option<&str>,
    label: &str,
) -> Result<usize> {
    let value = field(line, start, end).trim();
    if !value.is_empty() {
        return value
            .parse::<usize>()
            .with_context(|| format!("invalid PDB {label}"));
    }

    fallback
        .ok_or_else(|| anyhow!("missing PDB {label}"))
        .and_then(|value| {
            value
                .parse::<usize>()
                .with_context(|| format!("invalid PDB {label}"))
        })
}

pub(super) fn parse_fixed_width_i32_or_default(
    line: &str,
    start: usize,
    end: usize,
    label: &str,
    default: i32,
) -> Result<i32> {
    let value = field(line, start, end).trim();
    if value.is_empty() {
        Ok(default)
    } else {
        value
            .parse::<i32>()
            .with_context(|| format!("invalid PDB {label}"))
    }
}

pub(super) fn field(line: &str, start: usize, end: usize) -> &str {
    // PDB is a fixed-column format, but the columns are byte offsets: a stray
    // multibyte character in a malformed file would put a boundary mid-codepoint
    // and panic the slice. Clamp both ends down to a char boundary so a bad line
    // degrades to a short/empty field instead of crashing the reader.
    let end = floor_char_boundary(line, end.min(line.len()));
    let start = floor_char_boundary(line, start.min(end));
    &line[start..end]
}

/// The largest char boundary `<= index` (stable-Rust stand-in for the unstable
/// `str::floor_char_boundary`). A no-op for the ASCII columns of a well-formed PDB.
fn floor_char_boundary(line: &str, mut index: usize) -> usize {
    if index >= line.len() {
        return line.len();
    }
    while index > 0 && !line.is_char_boundary(index) {
        index -= 1;
    }
    index
}

pub(super) fn ordered_pair<T: Ord>(first: T, second: T) -> (T, T) {
    if first <= second {
        (first, second)
    } else {
        (second, first)
    }
}
