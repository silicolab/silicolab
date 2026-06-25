use anyhow::{Result, anyhow, bail};

struct Section {
    header_line: usize,
    last_content_line: usize,
}

pub(super) struct ProteinTopology {
    lines: Vec<String>,
    pub(super) atom_count: usize,
    pub(super) max_residue_number: i32,
    atoms_section: Section,
    bonds_section: Option<Section>,
    atom_records: Vec<AtomRecord>,
}

struct AtomRecord {
    line_index: usize,
    nr: usize,
    residue_number: i32,
    residue_name: String,
    atom_name: String,
    charge_token_range: std::ops::Range<usize>,
}

impl ProteinTopology {
    pub(super) fn parse(text: &str) -> Result<Self> {
        let lines: Vec<String> = text.lines().map(|line| line.to_string()).collect();

        let atoms_header = find_first_directive(&lines, "atoms")
            .ok_or_else(|| anyhow!("the protein topology has no [ atoms ] block"))?;
        let atoms_end = directive_block_end(&lines, atoms_header);

        let mut atom_records = Vec::new();
        let mut atom_count = 0usize;
        let mut max_residue_number = 0i32;
        let mut last_content_line = atoms_header;
        for (line_index, raw) in lines
            .iter()
            .enumerate()
            .take(atoms_end)
            .skip(atoms_header + 1)
        {
            let content = strip_comment(raw);
            if content.trim().is_empty() {
                continue;
            }
            if let Some(record) = parse_atom_record(raw, content, line_index) {
                atom_count = atom_count.max(record.nr);
                max_residue_number = max_residue_number.max(record.residue_number);
                last_content_line = line_index;
                atom_records.push(record);
            }
        }
        if atom_records.is_empty() {
            bail!("the protein [ atoms ] block has no atom rows");
        }
        for (expected, record) in atom_records.iter().enumerate() {
            if record.nr != expected + 1 {
                bail!(
                    "the protein [ atoms ] block must use contiguous 1-based atom numbers \
                     to merge a glycan (found nr {} at position {}); a multi-moleculetype or \
                     gapped topology is not supported",
                    record.nr,
                    expected + 1
                );
            }
        }
        if find_first_directive(&lines, "moleculetype")
            .map(|first| {
                lines[first + 1..]
                    .iter()
                    .any(|line| directive_name(strip_comment(line)) == Some("moleculetype"))
            })
            .unwrap_or(false)
        {
            bail!(
                "merging a glycan requires a single [ moleculetype ]; the protein topology \
                 declares more than one"
            );
        }

        let atoms_section = Section {
            header_line: atoms_header,
            last_content_line,
        };

        let bonds_section = find_first_directive(&lines, "bonds").map(|bonds_header| {
            let bonds_end = directive_block_end(&lines, bonds_header);
            let mut last = bonds_header;
            for (line_index, line) in lines
                .iter()
                .enumerate()
                .take(bonds_end)
                .skip(bonds_header + 1)
            {
                if !strip_comment(line).trim().is_empty() {
                    last = line_index;
                }
            }
            Section {
                header_line: bonds_header,
                last_content_line: last,
            }
        });

        Ok(ProteinTopology {
            lines,
            atom_count,
            max_residue_number,
            atoms_section,
            bonds_section,
            atom_records,
        })
    }

    pub(super) fn anchor_atom_index(
        &self,
        sequence_number: i32,
        residue_name: &str,
        atom_name: &str,
    ) -> Result<usize> {
        self.atom_records
            .iter()
            .find(|record| {
                record.residue_number == sequence_number
                    && record.residue_name.eq_ignore_ascii_case(residue_name)
                    && record.atom_name == atom_name
            })
            .or_else(|| {
                self.atom_records.iter().find(|record| {
                    record.residue_name.eq_ignore_ascii_case(residue_name)
                        && record.atom_name == atom_name
                })
            })
            .map(|record| record.nr)
            .ok_or_else(|| {
                anyhow!(
                    "could not locate anchor atom {residue_name}.{atom_name} (resnr {sequence_number}) in the protein topology"
                )
            })
    }

    pub(super) fn adjust_atom_charge(
        &mut self,
        sequence_number: i32,
        residue_name: &str,
        atom_name: &str,
        delta: f32,
    ) {
        let target = self
            .atom_records
            .iter()
            .position(|record| {
                record.residue_number == sequence_number
                    && record.residue_name.eq_ignore_ascii_case(residue_name)
                    && record.atom_name == atom_name
            })
            .or_else(|| {
                self.atom_records.iter().position(|record| {
                    record.residue_name.eq_ignore_ascii_case(residue_name)
                        && record.atom_name == atom_name
                })
            });
        let Some(target) = target else {
            return;
        };
        let record = &self.atom_records[target];
        let line = &self.lines[record.line_index];
        let range = record.charge_token_range.clone();
        let existing: f32 = line[range.clone()].trim().parse().unwrap_or(0.0);
        let updated = format!("{:.4}", existing + delta);
        let mut new_line = String::with_capacity(line.len() + updated.len());
        new_line.push_str(&line[..range.start]);
        new_line.push_str(&updated);
        new_line.push_str(&line[range.end..]);
        self.lines[record.line_index] = new_line;
        self.atom_records[target].charge_token_range = range.start..range.start + updated.len();
    }

    pub(super) fn total_charge(&self) -> f32 {
        self.atom_records
            .iter()
            .map(|record| {
                strip_comment(&self.lines[record.line_index])
                    .split_whitespace()
                    .nth(6)
                    .and_then(|token| token.parse::<f32>().ok())
                    .unwrap_or(0.0)
            })
            .sum()
    }

    pub(super) fn append_atoms(&mut self, atom_lines: &[String]) {
        let insert_at = self.atoms_section.last_content_line + 1;
        let block: Vec<String> = atom_lines
            .iter()
            .map(|line| line.trim_end_matches('\n').to_string())
            .collect();
        self.splice_after(insert_at, block);
    }

    pub(super) fn append_bonds(&mut self, bond_lines: &[String]) {
        let block: Vec<String> = bond_lines
            .iter()
            .map(|line| line.trim_end_matches('\n').to_string())
            .collect();
        match &self.bonds_section {
            Some(section) => {
                let insert_at = section.last_content_line + 1;
                self.splice_after(insert_at, block);
            }
            None => {
                let insert_at = self.atoms_section.last_content_line + 1;
                let mut new_block = vec![String::new(), "[ bonds ]".to_string()];
                new_block.extend(block);
                self.splice_after(insert_at, new_block);
            }
        }
    }

    /// Insert raw directive blocks (each line verbatim, including its own
    /// `[ … ]` headers) immediately after the molecule's `[ bonds ]` block — the
    /// home for the glycan's `[ pairs ]`/`[ angles ]`/`[ dihedrals ]`. The bonds
    /// block is located in the *current* line buffer so this is correct even
    /// after [`append_bonds`](Self::append_bonds) has grown it; GROMACS treats a
    /// second instance of a directive within a moleculetype as cumulative.
    pub(super) fn append_after_bonds(&mut self, lines: &[String]) {
        let block: Vec<String> = lines
            .iter()
            .map(|line| line.trim_end_matches('\n').to_string())
            .collect();
        if block.is_empty() {
            return;
        }
        let insert_at = match find_first_directive(&self.lines, "bonds") {
            Some(header) => {
                let end = directive_block_end(&self.lines, header);
                let mut last = header;
                for line_index in (header + 1)..end {
                    if !strip_comment(&self.lines[line_index]).trim().is_empty() {
                        last = line_index;
                    }
                }
                last + 1
            }
            None => self.atoms_section.last_content_line + 1,
        };
        self.splice_after(insert_at, block);
    }

    fn splice_after(&mut self, insert_at: usize, block: Vec<String>) {
        let added = block.len();
        let at = insert_at.min(self.lines.len());
        let tail: Vec<String> = self.lines.split_off(at);
        self.lines.extend(block);
        self.lines.extend(tail);
        self.shift_line_indices(at, added);
    }

    fn shift_line_indices(&mut self, at: usize, added: usize) {
        if self.atoms_section.header_line >= at {
            self.atoms_section.header_line += added;
        }
        if self.atoms_section.last_content_line >= at {
            self.atoms_section.last_content_line += added;
        }
        if let Some(section) = self.bonds_section.as_mut() {
            if section.header_line >= at {
                section.header_line += added;
            }
            if section.last_content_line >= at {
                section.last_content_line += added;
            }
        }
        for record in self.atom_records.iter_mut() {
            if record.line_index >= at {
                record.line_index += added;
            }
        }
    }

    pub(super) fn render(&self) -> String {
        let mut out = self.lines.join("\n");
        out.push('\n');
        out
    }
}

fn strip_comment(line: &str) -> &str {
    match line.find(';') {
        Some(idx) => &line[..idx],
        None => line,
    }
}

fn find_first_directive(lines: &[String], name: &str) -> Option<usize> {
    lines.iter().position(|line| {
        directive_name(strip_comment(line))
            .map(|directive| directive.eq_ignore_ascii_case(name))
            .unwrap_or(false)
    })
}

fn directive_name(line: &str) -> Option<&str> {
    let trimmed = line.trim();
    let inside = trimmed.strip_prefix('[')?;
    let inside = inside.strip_suffix(']')?;
    Some(inside.trim())
}

fn directive_block_end(lines: &[String], header: usize) -> usize {
    for (line_index, line) in lines.iter().enumerate().skip(header + 1) {
        if directive_name(strip_comment(line)).is_some() {
            return line_index;
        }
        if strip_comment(line).trim().starts_with("#include") {
            return line_index;
        }
        if line.trim_start().starts_with('#') {
            return line_index;
        }
    }
    lines.len()
}

fn parse_atom_record(raw: &str, content: &str, line_index: usize) -> Option<AtomRecord> {
    let cols: Vec<&str> = content.split_whitespace().collect();
    if cols.len() < 7 {
        return None;
    }
    let nr: usize = cols[0].parse().ok()?;
    let residue_number: i32 = cols[2].parse().ok()?;
    let residue_name = cols[3].to_string();
    let atom_name = cols[4].to_string();
    let charge_token = cols[6];
    let charge_token_range = locate_nth_token(raw, 6)?;
    if &raw[charge_token_range.clone()] != charge_token {
        return None;
    }
    Some(AtomRecord {
        line_index,
        nr,
        residue_number,
        residue_name,
        atom_name,
        charge_token_range,
    })
}

fn locate_nth_token(line: &str, n: usize) -> Option<std::ops::Range<usize>> {
    let bytes = line.as_bytes();
    let mut index = 0usize;
    let mut token = 0usize;
    while index < bytes.len() {
        while index < bytes.len() && bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        if index >= bytes.len() {
            break;
        }
        let start = index;
        while index < bytes.len() && !bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        if token == n {
            return Some(start..index);
        }
        token += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const SINGLE_CHAIN_TOP: &str = "\
[ moleculetype ]
Protein_chain_A  3

[ atoms ]
     1  NH1    1     ASN      N     1    -0.4700  14.0070
     2  CT1    1     ASN      CA    2     0.0700  12.0110

[ bonds ]
     1     2 1
";

    #[test]
    fn single_contiguous_moleculetype_parses() {
        let parsed = ProteinTopology::parse(SINGLE_CHAIN_TOP).expect("parses");
        assert_eq!(parsed.atom_count, 2);
    }

    #[test]
    fn non_contiguous_numbering_is_rejected() {
        let gapped = SINGLE_CHAIN_TOP.replace(
            "     2  CT1    1     ASN      CA",
            "     3  CT1    1     ASN      CA",
        );
        let err = match ProteinTopology::parse(&gapped) {
            Ok(_) => panic!("gapped nr must be rejected"),
            Err(err) => err,
        };
        assert!(err.to_string().contains("contiguous"));
    }

    const TWO_ASN_TOP: &str = "\
[ moleculetype ]
Protein_chain_A  3

[ atoms ]
     1  NH1  100   ASN      N     1    -0.4700  14.0070
     2  NH2  100   ASN      ND2   2    -0.6200  14.0070
     3  NH1  297   ASN      N     3    -0.4700  14.0070
     4  NH2  297   ASN      ND2   4    -0.6200  14.0070

[ bonds ]
     1     2 1
";

    #[test]
    fn anchor_lookup_disambiguates_by_preserved_residue_number() {
        let parsed = ProteinTopology::parse(TWO_ASN_TOP).expect("parses");
        let nr = parsed.anchor_atom_index(297, "ASN", "ND2").expect("found");
        assert_eq!(
            nr, 4,
            "must target the ND2 of residue 297, not the first ASN"
        );
    }

    fn nd2_charge(top: &str, resnr: &str) -> f32 {
        top.lines()
            .find(|line| {
                let c: Vec<&str> = line.split_whitespace().collect();
                c.len() >= 7 && c[2] == resnr && c[3] == "ASN" && c[4] == "ND2"
            })
            .and_then(|line| line.split_whitespace().nth(6).and_then(|t| t.parse().ok()))
            .unwrap()
    }

    #[test]
    fn charge_adjust_targets_the_numbered_residue_not_the_first() {
        let mut parsed = ProteinTopology::parse(TWO_ASN_TOP).expect("parses");
        parsed.adjust_atom_charge(297, "ASN", "ND2", 0.10);
        let rendered = parsed.render();
        assert!(
            (nd2_charge(&rendered, "297") - (-0.52)).abs() < 1e-4,
            "ASN 297 ND2 shifts"
        );
        assert!(
            (nd2_charge(&rendered, "100") - (-0.62)).abs() < 1e-4,
            "ASN 100 ND2 untouched"
        );
    }

    #[test]
    fn multiple_moleculetypes_are_rejected() {
        let multi = format!(
            "{SINGLE_CHAIN_TOP}\n[ moleculetype ]\nProtein_chain_B  3\n\n[ atoms ]\n     1  NH1    1     ASN      N     1    -0.4700  14.0070\n"
        );
        let err = match ProteinTopology::parse(&multi) {
            Ok(_) => panic!("multiple moleculetypes must be rejected"),
            Err(err) => err,
        };
        assert!(err.to_string().contains("single [ moleculetype ]"));
    }
}
