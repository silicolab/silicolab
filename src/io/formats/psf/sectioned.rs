use anyhow::{Result, anyhow, bail};

#[derive(Debug, Clone)]
pub struct PsfExtensionBlock {
    pub name: String,
    pub payload: PsfExtensionPayload,
}

#[derive(Debug, Clone)]
pub enum PsfExtensionPayload {
    Sectioned(PsfSectionedBlock),
}

#[derive(Debug, Clone)]
pub struct PsfSectionedBlock {
    pub sections: Vec<PsfSection>,
}

#[derive(Debug, Clone)]
pub enum PsfSection {
    Key(PsfKeyBlock),
    Table(PsfTableBlock),
}

#[derive(Debug, Clone)]
pub struct PsfKeyBlock {
    pub entries: Vec<PsfKeyEntry>,
}

#[derive(Debug, Clone)]
pub struct PsfKeyEntry {
    pub key: String,
    pub value: String,
}

#[derive(Debug, Clone)]
pub struct PsfTableBlock {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<String>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PsfSectionTag {
    Key,
    Table,
}

impl PsfSectionTag {
    fn parse(line: &str) -> Option<Self> {
        match line {
            "#KEY" => Some(Self::Key),
            "#TABLE" => Some(Self::Table),
            _ => None,
        }
    }
}

impl PsfSectionedBlock {
    pub(crate) fn key_value(&self, key: &str) -> Option<&str> {
        self.sections.iter().find_map(|section| match section {
            PsfSection::Key(block) => block.value(key),
            PsfSection::Table(_) => None,
        })
    }

    pub(crate) fn first_table(&self) -> Option<&PsfTableBlock> {
        self.sections.iter().find_map(|section| match section {
            PsfSection::Table(table) => Some(table),
            PsfSection::Key(_) => None,
        })
    }
}

impl PsfKeyBlock {
    fn value(&self, key: &str) -> Option<&str> {
        self.entries
            .iter()
            .find(|entry| entry.key.eq_ignore_ascii_case(key))
            .map(|entry| entry.value.as_str())
    }
}

pub(crate) fn parse_extension_block(
    name: &str,
    lines: &[&str],
    index: &mut usize,
) -> Result<PsfExtensionBlock> {
    while *index < lines.len() {
        let line = lines[*index].trim();
        if line.starts_with("@<") {
            break;
        }
        if line.is_empty() {
            *index += 1;
            continue;
        }

        if !line.starts_with('#') {
            bail!("PSF @<SILICOLAB>{name} must use section tags like #KEY or #TABLE");
        }

        let payload = PsfExtensionPayload::Sectioned(parse_sectioned_block(name, lines, index)?);
        return Ok(PsfExtensionBlock {
            name: name.to_string(),
            payload,
        });
    }

    bail!("PSF @<SILICOLAB>{name} block is empty")
}

fn parse_sectioned_block(
    name: &str,
    lines: &[&str],
    index: &mut usize,
) -> Result<PsfSectionedBlock> {
    let mut sections = Vec::new();

    while *index < lines.len() {
        let line = lines[*index].trim();
        if line.starts_with("@<") {
            break;
        }
        if line.is_empty() {
            *index += 1;
            continue;
        }

        let tag = PsfSectionTag::parse(line)
            .ok_or_else(|| anyhow!("PSF @<SILICOLAB>{name} has unsupported section tag {line}"))?;
        *index += 1;

        match tag {
            PsfSectionTag::Key => sections.push(PsfSection::Key(parse_key_section(lines, index))),
            PsfSectionTag::Table => {
                sections.push(PsfSection::Table(parse_table_section(name, lines, index)?))
            }
        }
    }

    if sections.is_empty() {
        bail!("PSF @<SILICOLAB>{name} block contains no sections");
    }

    Ok(PsfSectionedBlock { sections })
}

fn parse_key_section(lines: &[&str], index: &mut usize) -> PsfKeyBlock {
    let mut entries = Vec::new();

    while *index < lines.len() {
        let line = lines[*index].trim();
        if line.starts_with("@<") || line.starts_with('#') {
            break;
        }
        *index += 1;
        if line.is_empty() {
            continue;
        }

        let mut parts = line.split_whitespace();
        let Some(key) = parts.next() else {
            continue;
        };
        let value = parts.collect::<Vec<_>>().join(" ");
        entries.push(PsfKeyEntry {
            key: key.to_string(),
            value,
        });
    }

    PsfKeyBlock { entries }
}

fn parse_table_section(name: &str, lines: &[&str], index: &mut usize) -> Result<PsfTableBlock> {
    let Some(columns_line) = next_section_data_line(lines, index) else {
        bail!("PSF @<SILICOLAB>{name} #TABLE is missing columns");
    };
    let columns = columns_line
        .split_whitespace()
        .map(str::to_string)
        .collect::<Vec<_>>();
    if columns.is_empty() {
        bail!("PSF @<SILICOLAB>{name} #TABLE has no columns");
    }

    let mut rows = Vec::new();
    while *index < lines.len() {
        let line = lines[*index].trim();
        if line.starts_with("@<") || line.starts_with('#') {
            break;
        }
        *index += 1;
        if line.is_empty() {
            continue;
        }

        let fields = line
            .split_whitespace()
            .map(str::to_string)
            .collect::<Vec<_>>();
        if fields.len() != columns.len() {
            bail!(
                "PSF @<SILICOLAB>{name} #TABLE row has {} fields but header defines {} columns",
                fields.len(),
                columns.len()
            );
        }
        rows.push(fields);
    }

    Ok(PsfTableBlock { columns, rows })
}

fn next_section_data_line<'a>(lines: &'a [&'a str], index: &mut usize) -> Option<&'a str> {
    while *index < lines.len() {
        let line = lines[*index].trim();
        if line.starts_with("@<") || line.starts_with('#') {
            return None;
        }
        *index += 1;
        if line.is_empty() {
            continue;
        }
        return Some(line);
    }

    None
}

pub(crate) fn required_column_index(table: &PsfTableBlock, column: &str) -> Result<usize> {
    table
        .columns
        .iter()
        .position(|entry| entry.eq_ignore_ascii_case(column))
        .ok_or_else(|| anyhow!("PSF #TABLE block is missing required column {column}"))
}
