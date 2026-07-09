use anyhow::{Result, anyhow, bail};

#[derive(Debug, Clone)]
pub struct SlfExtensionBlock {
    pub name: String,
    pub payload: SlfExtensionPayload,
}

#[derive(Debug, Clone)]
pub enum SlfExtensionPayload {
    Sectioned(SlfSectionedBlock),
}

#[derive(Debug, Clone)]
pub struct SlfSectionedBlock {
    pub sections: Vec<SlfSection>,
}

#[derive(Debug, Clone)]
pub enum SlfSection {
    Key(SlfKeyBlock),
    Table(SlfTableBlock),
}

#[derive(Debug, Clone)]
pub struct SlfKeyBlock {
    pub entries: Vec<SlfKeyEntry>,
}

#[derive(Debug, Clone)]
pub struct SlfKeyEntry {
    pub key: String,
    pub value: String,
}

#[derive(Debug, Clone)]
pub struct SlfTableBlock {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<String>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SlfSectionTag {
    Key,
    Table,
}

impl SlfSectionTag {
    fn parse(line: &str) -> Option<Self> {
        match line {
            "#KEY" => Some(Self::Key),
            "#TABLE" => Some(Self::Table),
            _ => None,
        }
    }
}

impl SlfSectionedBlock {
    pub(crate) fn key_value(&self, key: &str) -> Option<&str> {
        self.sections.iter().find_map(|section| match section {
            SlfSection::Key(block) => block.value(key),
            SlfSection::Table(_) => None,
        })
    }

    pub(crate) fn first_table(&self) -> Option<&SlfTableBlock> {
        self.sections.iter().find_map(|section| match section {
            SlfSection::Table(table) => Some(table),
            SlfSection::Key(_) => None,
        })
    }
}

impl SlfKeyBlock {
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
) -> Result<SlfExtensionBlock> {
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
            bail!("SLF @<SILICOLAB>{name} must use section tags like #KEY or #TABLE");
        }

        let payload = SlfExtensionPayload::Sectioned(parse_sectioned_block(name, lines, index)?);
        return Ok(SlfExtensionBlock {
            name: name.to_string(),
            payload,
        });
    }

    bail!("SLF @<SILICOLAB>{name} block is empty")
}

fn parse_sectioned_block(
    name: &str,
    lines: &[&str],
    index: &mut usize,
) -> Result<SlfSectionedBlock> {
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

        let tag = SlfSectionTag::parse(line)
            .ok_or_else(|| anyhow!("SLF @<SILICOLAB>{name} has unsupported section tag {line}"))?;
        *index += 1;

        match tag {
            SlfSectionTag::Key => sections.push(SlfSection::Key(parse_key_section(lines, index))),
            SlfSectionTag::Table => {
                sections.push(SlfSection::Table(parse_table_section(name, lines, index)?))
            }
        }
    }

    if sections.is_empty() {
        bail!("SLF @<SILICOLAB>{name} block contains no sections");
    }

    Ok(SlfSectionedBlock { sections })
}

fn parse_key_section(lines: &[&str], index: &mut usize) -> SlfKeyBlock {
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
        entries.push(SlfKeyEntry {
            key: key.to_string(),
            value,
        });
    }

    SlfKeyBlock { entries }
}

fn parse_table_section(name: &str, lines: &[&str], index: &mut usize) -> Result<SlfTableBlock> {
    let Some(columns_line) = next_section_data_line(lines, index) else {
        bail!("SLF @<SILICOLAB>{name} #TABLE is missing columns");
    };
    let columns = columns_line
        .split_whitespace()
        .map(str::to_string)
        .collect::<Vec<_>>();
    if columns.is_empty() {
        bail!("SLF @<SILICOLAB>{name} #TABLE has no columns");
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
                "SLF @<SILICOLAB>{name} #TABLE row has {} fields but header defines {} columns",
                fields.len(),
                columns.len()
            );
        }
        rows.push(fields);
    }

    Ok(SlfTableBlock { columns, rows })
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

pub(crate) fn required_column_index(table: &SlfTableBlock, column: &str) -> Result<usize> {
    table
        .columns
        .iter()
        .position(|entry| entry.eq_ignore_ascii_case(column))
        .ok_or_else(|| anyhow!("SLF #TABLE block is missing required column {column}"))
}
