use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

pub const CHARMM36_TOKEN: &str = "charmm36";

macro_rules! charmm36_file {
    ($name:literal) => {
        (
            $name,
            include_str!(concat!(
                "../../../../assets/forcefields/charmm36.ff/",
                $name
            )),
        )
    };
}

// MacKerell-lab CHARMM36/CGenFF (Feb 2026, CGenFF 5.0), MIT — see
// assets/forcefields/charmm36.ff/LICENSE and THIRD-PARTY-NOTICES.md. GROMACS
// does not ship CHARMM36, so we vendor it. Only the protein, carbohydrate,
// shared-parameter, TIP3P and ion files staged below are kept; the upstream
// lipid/nucleic-acid/CGenFF/ether/metal/silicate subsets are pruned.
const CHARMM36_FILES: &[(&str, &str)] = &[
    charmm36_file!("forcefield.itp"),
    charmm36_file!("ffnonbonded.itp"),
    charmm36_file!("ffbonded.itp"),
    charmm36_file!("ffmissingdihedrals.itp"),
    charmm36_file!("cmap.itp"),
    charmm36_file!("nbfix.itp"),
    charmm36_file!("atomtypes.atp"),
    charmm36_file!("carb.rtp"),
    charmm36_file!("carb.hdb"),
    charmm36_file!("carb.r2b"),
    charmm36_file!("carb.n.tdb"),
    charmm36_file!("carb.c.tdb"),
    charmm36_file!("aminoacids.rtp"),
    charmm36_file!("aminoacids.hdb"),
    charmm36_file!("aminoacids.r2b"),
    charmm36_file!("aminoacids.arn"),
    charmm36_file!("aminoacids.n.tdb"),
    charmm36_file!("aminoacids.c.tdb"),
    charmm36_file!("watermodels.dat"),
    charmm36_file!("tip3p.itp"),
    charmm36_file!("ions.itp"),
];

pub const GLYCAN_FORCE_FIELD_INCLUDES: &[&str] =
    &["ffnonbonded.itp", "ffbonded.itp", "cmap.itp", "nbfix.itp"];

/// Water (`SOL`) and ion (`NA`/`CL`) molecule-type files the self-contained
/// glycan topology must `#include`, so a later `solvate`/`genion` step can
/// resolve what it appends to `[ molecules ]` — as a pdb2gmx topology would.
const GLYCAN_SOLVENT_INCLUDES: &[&str] = &["tip3p.itp", "ions.itp"];

pub struct ForceFieldBundle {
    pub token: &'static str,
    files: &'static [(&'static str, &'static str)],
}

pub fn bundle(token: &str) -> Option<ForceFieldBundle> {
    if token == CHARMM36_TOKEN {
        Some(ForceFieldBundle {
            token: CHARMM36_TOKEN,
            files: CHARMM36_FILES,
        })
    } else {
        None
    }
}

impl ForceFieldBundle {
    pub fn dir_name(&self) -> String {
        format!("{}.ff", self.token)
    }

    pub fn file(&self, name: &str) -> Option<&'static str> {
        self.files
            .iter()
            .find(|(file_name, _)| *file_name == name)
            .map(|(_, contents)| *contents)
    }

    pub fn carb_rtp(&self) -> Option<&'static str> {
        self.file("carb.rtp")
    }
}

pub fn stage_forcefield(token: &str, working_dir: &Path) -> Result<PathBuf> {
    let bundle = bundle(token)
        .ok_or_else(|| anyhow::anyhow!("no bundled force field for token `{token}`"))?;
    let ff_dir = working_dir.join(bundle.dir_name());
    std::fs::create_dir_all(&ff_dir)
        .with_context(|| format!("creating force-field directory {}", ff_dir.display()))?;
    for (name, contents) in bundle.files {
        let path = ff_dir.join(name);
        std::fs::write(&path, contents).with_context(|| format!("staging {}", path.display()))?;
    }
    Ok(ff_dir)
}

pub fn glycan_force_field_includes(token: &str) -> Result<String> {
    let bundle = bundle(token)
        .ok_or_else(|| anyhow::anyhow!("no bundled force field for token `{token}`"))?;
    let dir = bundle.dir_name();
    let mut text = String::new();
    for name in GLYCAN_FORCE_FIELD_INCLUDES
        .iter()
        .chain(GLYCAN_SOLVENT_INCLUDES)
    {
        text.push_str(&format!("#include \"{dir}/{name}\"\n"));
    }
    Ok(text)
}

#[derive(Debug, Clone, PartialEq)]
pub struct AtomTyping {
    pub atom_type: String,
    pub charge: f32,
    pub charge_group: i32,
}

pub type TypingTable = HashMap<(String, String), AtomTyping>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BondedTypeDefaults {
    pub bond_func: i32,
    pub angle_func: i32,
    pub proper_func: i32,
    pub improper_func: i32,
    pub nrexcl: u32,
}

/// Four atom *names*, local to a residue, naming one improper-dihedral term as
/// listed in a residue's `[ impropers ]` block. The order is significant — it is
/// preserved when emitting the term so grompp resolves it against the
/// order-specific improper `[ dihedraltypes ]`.
pub type ImproperNames = [String; 4];

#[derive(Debug, Clone, PartialEq)]
pub struct CarbTopologyDatabase {
    pub typing: TypingTable,
    pub defaults: BondedTypeDefaults,
    /// Per-residue improper dihedrals, keyed by CHARMM residue name (e.g.
    /// `BGLCNA`). Unlike angles and proper dihedrals, impropers are *not*
    /// derivable from connectivity, so they are carried verbatim from the rtp.
    pub impropers: HashMap<String, Vec<ImproperNames>>,
}

const SUBSECTIONS: &[&str] = &[
    "atoms",
    "bonds",
    "impropers",
    "dihedrals",
    "exclusions",
    "cmap",
    "angles",
    "bondedtypes",
];

fn section_name(line: &str) -> Option<&str> {
    let inside = line.strip_prefix('[')?;
    let inside = inside.strip_suffix(']')?;
    Some(inside.trim())
}

fn strip_comment(line: &str) -> &str {
    match line.find(';') {
        Some(idx) => &line[..idx],
        None => line,
    }
}

fn is_subsection(name: &str) -> bool {
    SUBSECTIONS.iter().any(|s| s.eq_ignore_ascii_case(name))
}

pub fn parse_carb_rtp(text: &str) -> Result<CarbTopologyDatabase> {
    let mut typing = TypingTable::new();
    let mut defaults: Option<BondedTypeDefaults> = None;
    let mut impropers: HashMap<String, Vec<ImproperNames>> = HashMap::new();

    let mut current_residue: Option<String> = None;
    let mut in_atoms = false;
    let mut in_bondedtypes = false;
    let mut in_impropers = false;

    for raw in text.lines() {
        let line = strip_comment(raw).trim();
        if line.is_empty() {
            continue;
        }

        if let Some(name) = section_name(line) {
            if is_subsection(name) {
                in_atoms = name.eq_ignore_ascii_case("atoms");
                in_bondedtypes = name.eq_ignore_ascii_case("bondedtypes");
                in_impropers = name.eq_ignore_ascii_case("impropers");
            } else {
                current_residue = Some(name.to_string());
                in_atoms = false;
                in_bondedtypes = false;
                in_impropers = false;
            }
            continue;
        }

        if in_impropers && let Some(residue) = &current_residue {
            let cols: Vec<&str> = line.split_whitespace().collect();
            if cols.len() >= 4 {
                impropers.entry(residue.clone()).or_default().push([
                    cols[0].to_string(),
                    cols[1].to_string(),
                    cols[2].to_string(),
                    cols[3].to_string(),
                ]);
            }
            continue;
        }

        if in_bondedtypes {
            let cols: Vec<&str> = line.split_whitespace().collect();
            if cols.len() >= 6 {
                defaults = Some(BondedTypeDefaults {
                    bond_func: cols[0].parse().unwrap_or(1),
                    angle_func: cols[1].parse().unwrap_or(5),
                    proper_func: cols[2].parse().unwrap_or(9),
                    improper_func: cols[3].parse().unwrap_or(2),
                    nrexcl: cols[5].parse().unwrap_or(3),
                });
            }
            continue;
        }

        if in_atoms && let Some(residue) = &current_residue {
            let cols: Vec<&str> = line.split_whitespace().collect();
            if cols.len() >= 4 {
                let atom_name = cols[0].to_string();
                let atom_type = cols[1].to_string();
                let charge = cols[2]
                    .parse::<f32>()
                    .with_context(|| format!("parsing charge for {residue}.{atom_name}"))?;
                let charge_group = cols[3].parse::<i32>().unwrap_or(1);
                typing.insert(
                    (residue.clone(), atom_name),
                    AtomTyping {
                        atom_type,
                        charge,
                        charge_group,
                    },
                );
            }
        }
    }

    let defaults =
        defaults.ok_or_else(|| anyhow::anyhow!("carb.rtp has no [ bondedtypes ] defaults row"))?;

    if typing.is_empty() {
        bail!("carb.rtp produced an empty typing table");
    }

    Ok(CarbTopologyDatabase {
        typing,
        defaults,
        impropers,
    })
}

pub fn charmm36_carb_database() -> Result<CarbTopologyDatabase> {
    let bundle =
        bundle(CHARMM36_TOKEN).ok_or_else(|| anyhow::anyhow!("CHARMM36 bundle missing"))?;
    let carb = bundle
        .carb_rtp()
        .ok_or_else(|| anyhow::anyhow!("CHARMM36 bundle has no carb.rtp"))?;
    parse_carb_rtp(carb)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
[ bondedtypes ]
; comment
    1       5        9          2            1           3      1       0

[ AGLC ]
; 4C1 alpha-D-glucose
  [ atoms ]
       C1   CC3162  0.3400   1
       H1     HCA1  0.0900   1
       O1    OC311 -0.6500   1
      HO1     HCP1  0.4200   1
       C5   CC3163  0.1100   1
  [ bonds ]
       C1    O1
       C1    H1

[ AGLCNA ]
; alpha-GlcNAc
  [ atoms ]
       C1   CC3162  0.3400   1
        N    NC2D1 -0.4700   2
       HN     HCP1  0.3100   2
  [ impropers ]
        C    CT     N     O
";

    #[test]
    fn parses_bondedtypes_defaults() {
        let db = parse_carb_rtp(SAMPLE).unwrap();
        assert_eq!(db.defaults.bond_func, 1);
        assert_eq!(db.defaults.angle_func, 5);
        assert_eq!(db.defaults.proper_func, 9);
        assert_eq!(db.defaults.improper_func, 2);
        assert_eq!(db.defaults.nrexcl, 3);
    }

    #[test]
    fn typing_keys_on_residue_and_atom() {
        let db = parse_carb_rtp(SAMPLE).unwrap();
        let c1 = db
            .typing
            .get(&("AGLC".to_string(), "C1".to_string()))
            .unwrap();
        assert_eq!(c1.atom_type, "CC3162");
        assert!((c1.charge - 0.34).abs() < 1e-4);
        assert_eq!(c1.charge_group, 1);

        let o1 = db
            .typing
            .get(&("AGLC".to_string(), "O1".to_string()))
            .unwrap();
        assert_eq!(o1.atom_type, "OC311");
        assert!((o1.charge + 0.65).abs() < 1e-4);
    }

    #[test]
    fn subsection_headers_do_not_start_a_residue() {
        let db = parse_carb_rtp(SAMPLE).unwrap();
        assert!(
            db.typing
                .keys()
                .all(|(residue, _)| residue == "AGLC" || residue == "AGLCNA")
        );
    }

    #[test]
    fn nitrogen_atom_named_n_is_parsed() {
        let db = parse_carb_rtp(SAMPLE).unwrap();
        let n = db
            .typing
            .get(&("AGLCNA".to_string(), "N".to_string()))
            .unwrap();
        assert_eq!(n.atom_type, "NC2D1");
    }

    #[test]
    fn impropers_are_captured_per_residue_in_listed_order() {
        let db = parse_carb_rtp(SAMPLE).unwrap();
        let aglcna = db.impropers.get("AGLCNA").expect("AGLCNA has impropers");
        assert_eq!(
            aglcna,
            &vec![[
                "C".to_string(),
                "CT".to_string(),
                "N".to_string(),
                "O".to_string()
            ]]
        );
        // A residue with no [ impropers ] block contributes no entry.
        assert!(!db.impropers.contains_key("AGLC"));
    }

    #[test]
    fn bundled_carb_rtp_carries_known_impropers() {
        let db = charmm36_carb_database().unwrap();
        // The N-acetyl amide of GlcNAc keeps its carbonyl planar.
        let bglcna = db.impropers.get("BGLCNA").expect("BGLCNA has impropers");
        assert!(
            bglcna
                .iter()
                .any(|imp| imp == &["C", "CT", "N", "O"].map(str::to_string)),
            "expected the amide improper C-CT-N-O, got {bglcna:?}"
        );
        // The uronate carboxylate of glucuronic acid stays planar too.
        let bdp = db.impropers.get("BGLCA").expect("BGLCA has impropers");
        assert!(
            bdp.iter()
                .any(|imp| imp[0] == "C6" && imp.contains(&"C5".to_string())),
            "expected a carboxylate improper on C6, got {bdp:?}"
        );
    }

    #[test]
    fn bundled_charmm36_carb_rtp_parses() {
        let db = charmm36_carb_database().unwrap();
        let c1 = db
            .typing
            .get(&("AGLC".to_string(), "C1".to_string()))
            .unwrap();
        assert_eq!(c1.atom_type, "CC3162");
        let gn = db
            .typing
            .get(&("BGLCNA".to_string(), "N".to_string()))
            .unwrap();
        assert_eq!(gn.atom_type, "NC2D1");
        assert_eq!(db.defaults.nrexcl, 3);
    }

    #[test]
    fn glycan_includes_reference_the_staged_dir() {
        let includes = glycan_force_field_includes(CHARMM36_TOKEN).unwrap();
        assert!(includes.contains(&format!("{CHARMM36_TOKEN}.ff/ffnonbonded.itp")));
        assert!(includes.contains("cmap.itp"));
        assert!(!includes.contains("forcefield.itp"));
        // Water and ions, so a later solvate/genion's `SOL`/`NA`/`CL` resolve.
        assert!(includes.contains(&format!("{CHARMM36_TOKEN}.ff/tip3p.itp")));
        assert!(includes.contains(&format!("{CHARMM36_TOKEN}.ff/ions.itp")));
    }

    #[test]
    fn staging_writes_the_force_field_directory() {
        let dir = std::env::temp_dir().join("silicolab_ff_stage_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let ff_dir = stage_forcefield(CHARMM36_TOKEN, &dir).unwrap();
        assert!(ff_dir.join("ffnonbonded.itp").exists());
        assert!(ff_dir.join("carb.rtp").exists());
        assert!(ff_dir.join("forcefield.itp").exists());
        assert!(ff_dir.join("aminoacids.rtp").exists());
        assert!(ff_dir.join("aminoacids.hdb").exists());
        assert!(ff_dir.join("aminoacids.n.tdb").exists());
        assert!(ff_dir.join("aminoacids.c.tdb").exists());
    }

    #[test]
    fn unknown_token_has_no_bundle() {
        assert!(bundle("amber99sb-ildn").is_none());
    }
}
