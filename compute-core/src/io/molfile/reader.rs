use std::collections::{BTreeSet, HashMap};

use anyhow::{Context, Result, anyhow, bail};
use nalgebra::Point2;

use super::MolfileSketch;
use super::syntax::{Record, collect_records, key_value, parse_list, split_fields, unquote};
use crate::domain::BondType;
use crate::domain::chemistry::is_element_symbol;
use crate::domain::sketch::{
    Bracket, RepeatConnectivity, RepeatUnit, Sketch, SketchAtom, SketchBond, implicit_hydrogens,
    seed_layout,
};

const MAX_CHARGE: i32 = 15;
const VALENCE_RANGE: std::ops::RangeInclusive<i32> = -1..=14;

/// Parse a V3000 molfile into a sketch.
pub fn read_molfile(text: &str) -> Result<MolfileSketch> {
    let lines: Vec<&str> = text.lines().collect();
    let Some(counts) = lines.get(3) else {
        bail!("molfile is shorter than its four header lines");
    };
    if counts.contains("V2000") {
        bail!("this is a V2000 molfile; only V3000 is supported");
    }
    if !counts.contains("V3000") {
        bail!("line 4 is not a V3000 counts line");
    }

    let parsed = collect_records(&lines, 4)?;
    let mut reader = Reader::default();
    let mut records = parsed.records.iter();
    while let Some(record) = records.next() {
        reader
            .top_level(record, &mut records)
            .with_context(|| format!("molfile line {}", record.line))?;
    }
    if parsed.ignored_lines > 0 {
        reader.warn("ignored lines outside V3000 records");
    }
    if parsed
        .trailing
        .iter()
        .any(|line| !line.trim().is_empty() && line.trim() != "$$$$")
    {
        reader.warn("read only the first molecule; ignored the data after `M  END`");
    }

    let title = lines[0].trim().to_string();
    let (sketch, warnings) = reader.finish()?;
    Ok(MolfileSketch {
        title,
        sketch,
        warnings,
    })
}

struct DeclaredCounts {
    atoms: usize,
    bonds: usize,
    sgroups: usize,
}

struct Copolymer {
    number: String,
    atoms: BTreeSet<usize>,
}

#[derive(Default)]
struct Reader {
    sketch: Sketch,
    counts: Option<DeclaredCounts>,
    atom_ids: HashMap<String, usize>,
    bond_ids: HashMap<String, usize>,
    /// Requested `VAL`, resolved once all bonds are known.
    valences: Vec<(usize, i32)>,
    sgroup_numbers: BTreeSet<String>,
    copolymers: Vec<Copolymer>,
    /// `PARENT` values of SRU groups.
    sru_parents: Vec<String>,
    /// Units whose brackets were absent from the file.
    unbracketed: Vec<usize>,
    warnings: Vec<String>,
}

impl Reader {
    fn warn(&mut self, message: impl Into<String>) {
        let message = message.into();
        if !self.warnings.contains(&message) {
            self.warnings.push(message);
        }
    }

    fn top_level<'a>(
        &mut self,
        record: &Record,
        rest: &mut impl Iterator<Item = &'a Record>,
    ) -> Result<()> {
        let fields = split_fields(&record.text)?;
        let words: Vec<String> = fields.iter().map(|f| f.to_ascii_uppercase()).collect();
        match words.iter().map(String::as_str).collect::<Vec<_>>()[..] {
            [] | ["END", "CTAB"] => Ok(()),
            ["BEGIN", "CTAB", ..] => Ok(()),
            ["COUNTS", ..] => self.counts_record(&fields),
            ["BEGIN", "ATOM"] => self.block(rest, "ATOM", Self::atom_record),
            ["BEGIN", "BOND"] => self.block(rest, "BOND", Self::bond_record),
            ["BEGIN", "SGROUP"] => self.block(rest, "SGROUP", Self::sgroup_record),
            ["BEGIN", "COLLECTION"] => {
                self.warn("ignored the COLLECTION block (stereo groups and atom lists)");
                skip_block(rest, "COLLECTION")
            }
            ["BEGIN", "OBJ3D"] => {
                self.warn("ignored the OBJ3D block");
                skip_block(rest, "OBJ3D")
            }
            ["BEGIN", "RGROUP", ..] => bail!("R-group blocks are not supported"),
            ["LINKNODE", ..] => bail!("link nodes are not supported"),
            _ => bail!("unsupported record `{}`", record.text),
        }
    }

    fn block<'a>(
        &mut self,
        rest: &mut impl Iterator<Item = &'a Record>,
        name: &str,
        mut handle: impl FnMut(&mut Self, &[String]) -> Result<()>,
    ) -> Result<()> {
        for record in rest.by_ref() {
            let fields = split_fields(&record.text)
                .with_context(|| format!("molfile line {}", record.line))?;
            if is_block_end(&fields, name) {
                return Ok(());
            }
            handle(self, &fields).with_context(|| format!("molfile line {}", record.line))?;
        }
        bail!("{name} block is not closed")
    }

    fn counts_record(&mut self, fields: &[String]) -> Result<()> {
        let [_, atoms, bonds, sgroups, ..] = fields else {
            bail!("COUNTS needs atom, bond, and Sgroup counts");
        };
        self.counts = Some(DeclaredCounts {
            atoms: atoms.parse().context("atom count is not a number")?,
            bonds: bonds.parse().context("bond count is not a number")?,
            sgroups: sgroups.parse().context("Sgroup count is not a number")?,
        });
        Ok(())
    }

    fn atom_record(&mut self, fields: &[String]) -> Result<()> {
        let [id, kind, x, y, z, map, properties @ ..] = fields else {
            bail!("atom record needs an id, type, x, y, z, and map");
        };
        let pos = Point2::new(coordinate(x)?, coordinate(y)?);
        if coordinate(z)? != 0.0 {
            self.warn("flattened nonzero z coordinates into 2D");
        }
        if map != "0" {
            self.warn("ignored atom-atom mapping");
        }

        let atom = match kind.as_str() {
            "*" => SketchAtom::attachment(pos),
            "R" | "R#" => {
                self.warn("read R atoms as attachment points");
                SketchAtom::attachment(pos)
            }
            kind if is_element_type(kind) => SketchAtom::new(kind, pos),
            "A" | "Q" => bail!("query atom `{kind}` is not supported"),
            kind => bail!("unsupported atom type `{kind}`"),
        };
        let index = self.sketch.atoms.len();
        self.sketch.atoms.push(atom);
        if self.atom_ids.insert(id.clone(), index).is_some() {
            bail!("duplicate atom id {id}");
        }
        for field in properties {
            self.atom_property(index, field)?;
        }
        Ok(())
    }

    fn atom_property(&mut self, index: usize, field: &str) -> Result<()> {
        let (key, value) = key_value(field)?;
        match key.as_str() {
            "CHG" => {
                let charge: i32 = value.parse().context("CHG is not a number")?;
                if charge.abs() > MAX_CHARGE {
                    bail!("charge {charge} is out of range");
                }
                self.sketch.atoms[index].charge = charge;
            }
            "VAL" => {
                let valence: i32 = value.parse().context("VAL is not a number")?;
                if !VALENCE_RANGE.contains(&valence) {
                    bail!("VAL {valence} is out of range");
                }
                self.valences.push((index, valence));
            }
            "RGROUPS" if self.sketch.atoms[index].is_attachment() => {}
            "CFG" => self.warn("ignored atom stereo (CFG)"),
            "MASS" => self.warn("ignored isotope masses (MASS)"),
            "RAD" => self.warn("ignored radical states (RAD)"),
            other => self.warn(format!("ignored atom property {other}")),
        }
        Ok(())
    }

    fn bond_record(&mut self, fields: &[String]) -> Result<()> {
        let [id, kind, a, b, properties @ ..] = fields else {
            bail!("bond record needs an id, type, and two atoms");
        };
        let order = match kind.as_str() {
            "1" => BondType::Single,
            "2" => BondType::Double,
            "3" => BondType::Triple,
            "4" => BondType::Aromatic,
            kind => bail!("unsupported bond type {kind}"),
        };
        let a = self.atom_index(a)?;
        let b = self.atom_index(b)?;
        if a == b {
            bail!("bond {id} joins an atom to itself");
        }
        if self.sketch.bond_between(a, b).is_some() {
            bail!("bond {id} duplicates an existing bond");
        }
        for field in properties {
            let (key, _) = key_value(field)?;
            match key.as_str() {
                "ENDPTS" | "ATTACH" => bail!("variable-attachment bonds are not supported"),
                "CFG" => self.warn("ignored bond stereo (CFG)"),
                other => self.warn(format!("ignored bond property {other}")),
            }
        }
        if self
            .bond_ids
            .insert(id.clone(), self.sketch.bonds.len())
            .is_some()
        {
            bail!("duplicate bond id {id}");
        }
        self.sketch.bonds.push(SketchBond { a, b, order });
        Ok(())
    }

    fn atom_index(&self, id: &str) -> Result<usize> {
        self.atom_ids
            .get(id)
            .copied()
            .ok_or_else(|| anyhow!("unknown atom id {id}"))
    }

    fn sgroup_record(&mut self, fields: &[String]) -> Result<()> {
        let [number, kind, _external, properties @ ..] = fields else {
            bail!("Sgroup record needs an index, type, and external index");
        };
        if !self.sgroup_numbers.insert(number.clone()) {
            bail!("duplicate Sgroup index {number}");
        }
        match kind.to_ascii_uppercase().as_str() {
            "SRU" => self.repeat_unit(number, properties),
            "COP" => self.copolymer(number, properties),
            other => bail!("Sgroup type {other} is not supported"),
        }
    }

    fn repeat_unit(&mut self, number: &str, properties: &[String]) -> Result<()> {
        let mut atoms = None;
        let mut crossing = None;
        let mut label = String::new();
        let mut connectivity = RepeatConnectivity::default();
        let mut brackets = Vec::new();
        let mut parent = None;
        for field in properties {
            let (key, value) = key_value(field)?;
            match key.as_str() {
                "ATOMS" => atoms = Some(self.atom_list(value)?),
                "XBONDS" => crossing = Some(self.crossing_bonds(number, value)?),
                "LABEL" => label = unquote(value),
                "CONNECT" => connectivity = parse_connectivity(value)?,
                "BRKXYZ" => brackets.push(parse_bracket(value)?),
                "BRKTYP" if unquote(value).eq_ignore_ascii_case("BRACKET") => {}
                "PARENT" => parent = Some(value.to_string()),
                other => self.warn(format!("ignored SRU property {other}")),
            }
        }
        self.sru_parents.extend(parent);
        let atoms = atoms.ok_or_else(|| anyhow!("SRU {number} has no ATOMS"))?;
        let crossing_bonds = crossing.ok_or_else(|| anyhow!("SRU {number} has no XBONDS"))?;
        if brackets.is_empty() {
            self.unbracketed.push(self.sketch.repeat_units.len());
        }
        self.sketch.repeat_units.push(RepeatUnit {
            atoms,
            crossing_bonds,
            label,
            connectivity,
            brackets,
        });
        Ok(())
    }

    fn crossing_bonds(&self, number: &str, value: &str) -> Result<[usize; 2]> {
        let ids = parse_list(value).context("bad XBONDS")?;
        let [first, second] = ids[..] else {
            bail!(
                "SRU {number} has {} crossing bonds; exactly 2 are supported",
                ids.len()
            );
        };
        Ok([self.bond_index(first)?, self.bond_index(second)?])
    }

    fn bond_index(&self, id: &str) -> Result<usize> {
        self.bond_ids
            .get(id)
            .copied()
            .ok_or_else(|| anyhow!("unknown bond id {id}"))
    }

    fn atom_list(&self, value: &str) -> Result<Vec<usize>> {
        parse_list(value)
            .context("bad ATOMS")?
            .into_iter()
            .map(|id| self.atom_index(id))
            .collect()
    }

    fn copolymer(&mut self, number: &str, properties: &[String]) -> Result<()> {
        let mut atoms = BTreeSet::new();
        for field in properties {
            let (key, value) = key_value(field)?;
            if key == "ATOMS" {
                atoms = self.atom_list(value)?.into_iter().collect();
            }
        }
        self.copolymers.push(Copolymer {
            number: number.to_string(),
            atoms,
        });
        Ok(())
    }

    fn finish(mut self) -> Result<(Sketch, Vec<String>)> {
        let copolymers = std::mem::take(&mut self.copolymers);
        for copolymer in &copolymers {
            let wraps_units = self
                .sketch
                .repeat_units
                .iter()
                .any(|unit| unit.atoms.iter().all(|atom| copolymer.atoms.contains(atom)));
            if !wraps_units {
                bail!(
                    "COP Sgroup {} has no SRU inside it; only the SRU groups of a copolymer \
                     are supported",
                    copolymer.number
                );
            }
            self.warn("dropped the COP wrapper and kept its SRU groups");
        }

        let nested = std::mem::take(&mut self.sru_parents)
            .iter()
            .any(|parent| !copolymers.iter().any(|cop| cop.number == *parent));
        if nested {
            self.warn("flattened nested SRU groups (PARENT)");
        }

        if self.sketch.atoms.is_empty() {
            bail!("molfile contains no atoms");
        }
        let counts = self
            .counts
            .as_ref()
            .ok_or_else(|| anyhow!("molfile has no COUNTS record"))?;
        let found = (
            self.sketch.atoms.len(),
            self.sketch.bonds.len(),
            self.sgroup_numbers.len(),
        );
        if (counts.atoms, counts.bonds, counts.sgroups) != found {
            bail!(
                "COUNTS declares {} atoms, {} bonds, {} Sgroups but the file has {}, {}, {}",
                counts.atoms,
                counts.bonds,
                counts.sgroups,
                found.0,
                found.1,
                found.2
            );
        }

        for (index, valence) in std::mem::take(&mut self.valences) {
            self.apply_valence(index, valence);
        }
        self.sketch.validate_repeat_units()?;

        let first = self.sketch.atoms[0].pos;
        if self.sketch.atoms.len() > 1 && self.sketch.atoms.iter().all(|atom| atom.pos == first) {
            seed_layout(&mut self.sketch);
            self.sketch.refit_repeat_unit_brackets();
            self.warn("the file has no 2D coordinates; generated a layout");
        } else {
            for unit in std::mem::take(&mut self.unbracketed) {
                self.sketch.repeat_units[unit].brackets =
                    self.sketch.fitted_brackets(&self.sketch.repeat_units[unit]);
            }
        }
        Ok((self.sketch, self.warnings))
    }

    /// Pin the hydrogens a `VAL` implies when they differ from the valence model.
    fn apply_valence(&mut self, index: usize, valence: i32) {
        let atom = &self.sketch.atoms[index];
        if atom.is_attachment() || valence == 0 {
            return;
        }
        let sum = self.sketch.bond_order_sum(index);
        let total = valence.max(0);
        let hydrogens = (total - sum.round() as i32).max(0) as u32;
        if hydrogens != implicit_hydrogens(&atom.element, atom.charge, sum) {
            self.sketch.atoms[index].explicit_hydrogens = Some(hydrogens);
        }
    }
}

fn is_block_end(fields: &[String], name: &str) -> bool {
    matches!(fields, [end, block] if end.eq_ignore_ascii_case("END") && block.eq_ignore_ascii_case(name))
}

fn skip_block<'a>(rest: &mut impl Iterator<Item = &'a Record>, name: &str) -> Result<()> {
    for record in rest.by_ref() {
        if is_block_end(&split_fields(&record.text)?, name) {
            return Ok(());
        }
    }
    bail!("{name} block is not closed")
}

/// Whether an atom type is an element. Types are case-sensitive (`CL` is not
/// chlorine) and pseudoatoms such as `Ala` must not match on their first letters.
fn is_element_type(kind: &str) -> bool {
    let mut chars = kind.chars();
    kind.len() <= 2
        && chars.next().is_some_and(|c| c.is_ascii_uppercase())
        && chars.all(|c| c.is_ascii_lowercase())
        && kind != "D"
        && kind != "T"
        && is_element_symbol(kind)
}

fn coordinate(value: &str) -> Result<f32> {
    let parsed: f32 = value
        .parse()
        .with_context(|| format!("bad coordinate `{value}`"))?;
    if !parsed.is_finite() {
        bail!("coordinate `{value}` is not finite");
    }
    Ok(parsed)
}

fn parse_connectivity(value: &str) -> Result<RepeatConnectivity> {
    match value.to_ascii_uppercase().as_str() {
        "HT" => Ok(RepeatConnectivity::HeadToTail),
        "HH" => Ok(RepeatConnectivity::HeadToHead),
        "EU" => Ok(RepeatConnectivity::EitherUnknown),
        other => bail!("unknown CONNECT value {other}"),
    }
}

fn parse_bracket(value: &str) -> Result<Bracket> {
    let items = parse_list(value).context("bad BRKXYZ")?;
    let [x1, y1, _, x2, y2, _, _, _, _] = items[..] else {
        bail!("BRKXYZ needs 9 numbers, found {}", items.len());
    };
    Ok([
        Point2::new(coordinate(x1)?, coordinate(y1)?),
        Point2::new(coordinate(x2)?, coordinate(y2)?),
    ])
}
