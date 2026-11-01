//! Read the GROMACS include topology acpype writes (`<base>_GMX.itp`) into a
//! [`TypedMolecule`]. Only the sections a single molecule carries are read;
//! `#include`, `[ defaults ]` and `[ system ]` belong to the `.top` and are
//! ignored if present.

use anyhow::{Context, Result, bail};

use crate::md::{AtomTypeParams, BondedInteraction, TypedAtom, TypedMolecule};

pub fn parse_itp(text: &str, source: &str) -> Result<TypedMolecule> {
    let mut molecule = TypedMolecule {
        name: String::new(),
        nrexcl: 3,
        source: source.to_string(),
        atom_types: Vec::new(),
        atoms: Vec::new(),
        bonds: Vec::new(),
        pairs: Vec::new(),
        angles: Vec::new(),
        dihedrals: Vec::new(),
    };
    let mut section = String::new();
    for (line_number, raw) in text.lines().enumerate() {
        let line = strip_comment(raw).trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(name) = section_name(line) {
            section = name.to_ascii_lowercase();
            continue;
        }
        let tokens: Vec<&str> = line.split_whitespace().collect();
        let context = || format!("line {} of the typed topology", line_number + 1);
        match section.as_str() {
            "atomtypes" => molecule
                .atom_types
                .push(parse_atom_type(&tokens).with_context(context)?),
            "moleculetype" => {
                let [name, nrexcl] = tokens[..] else {
                    bail!("{}: expected `name nrexcl`", context());
                };
                molecule.name = name.to_string();
                molecule.nrexcl = nrexcl.parse().with_context(context)?;
            }
            "atoms" => molecule
                .atoms
                .push(parse_atom(&tokens).with_context(context)?),
            "bonds" => molecule
                .bonds
                .push(parse_interaction(&tokens, 2).with_context(context)?),
            "pairs" => molecule
                .pairs
                .push(parse_interaction(&tokens, 2).with_context(context)?),
            "angles" => molecule
                .angles
                .push(parse_interaction(&tokens, 3).with_context(context)?),
            "dihedrals" => molecule
                .dihedrals
                .push(parse_interaction(&tokens, 4).with_context(context)?),
            _ => {}
        }
    }
    if molecule.atoms.is_empty() {
        bail!("the typed topology has no `[ atoms ]` section");
    }
    for atom in &molecule.atoms {
        if molecule.atom_type(&atom.type_name).is_none() {
            bail!(
                "atom {} uses type `{}` that the `[ atomtypes ]` section does not define",
                atom.name,
                atom.type_name
            );
        }
    }
    let atom_count = molecule.atoms.len() as u32;
    for interaction in molecule
        .bonds
        .iter()
        .chain(&molecule.pairs)
        .chain(&molecule.angles)
        .chain(&molecule.dihedrals)
    {
        if let Some(index) = interaction
            .atoms
            .iter()
            .find(|index| **index == 0 || **index > atom_count)
        {
            bail!("a bonded term refers to atom {index}, outside 1..={atom_count}");
        }
    }
    Ok(molecule)
}

/// `name bond_type mass charge ptype sigma epsilon` (acpype) or the shorter
/// `name mass charge ptype sigma epsilon` that hand-written fragments use.
fn parse_atom_type(tokens: &[&str]) -> Result<AtomTypeParams> {
    let (name, rest) = match tokens {
        [name, _bond_type, mass, charge, ptype, sigma, epsilon] => {
            (name, [mass, charge, ptype, sigma, epsilon])
        }
        [name, mass, charge, ptype, sigma, epsilon] => {
            (name, [mass, charge, ptype, sigma, epsilon])
        }
        _ => bail!("expected an atom-type row, got {} columns", tokens.len()),
    };
    let [mass, _charge, _ptype, sigma, epsilon] = rest;
    Ok(AtomTypeParams {
        name: name.to_string(),
        mass_u: number(mass)?,
        sigma_nm: number(sigma)?,
        epsilon_kj_mol: number(epsilon)?,
    })
}

/// `nr type resnr residue atom cgnr charge mass`; `mass` may be absent.
fn parse_atom(tokens: &[&str]) -> Result<TypedAtom> {
    let (type_name, residue_number, residue_name, name, charge, mass) = match tokens {
        [_, type_name, resnr, residue, atom, _cgnr, charge, mass, ..] => (
            type_name,
            resnr,
            residue,
            atom,
            number(charge)?,
            number(mass)?,
        ),
        [_, type_name, resnr, residue, atom, _cgnr, charge] => {
            (type_name, resnr, residue, atom, number(charge)?, 0.0)
        }
        _ => bail!("expected an atom row, got {} columns", tokens.len()),
    };
    Ok(TypedAtom {
        name: name.to_string(),
        type_name: type_name.to_string(),
        residue_name: residue_name.to_string(),
        residue_number: residue_number.parse()?,
        charge,
        mass_u: mass,
    })
}

fn parse_interaction(tokens: &[&str], atoms: usize) -> Result<BondedInteraction> {
    if tokens.len() < atoms + 1 {
        bail!("expected {atoms} atom indices and a function number");
    }
    let indices = tokens[..atoms]
        .iter()
        .map(|token| token.parse::<u32>().map_err(anyhow::Error::from))
        .collect::<Result<Vec<_>>>()?;
    let func = tokens[atoms].parse::<i32>()?;
    let params = tokens[atoms + 1..]
        .iter()
        .map(|token| number(token))
        .collect::<Result<Vec<_>>>()?;
    Ok(BondedInteraction {
        atoms: indices,
        func,
        params,
    })
}

fn number(token: &str) -> Result<f64> {
    token
        .parse::<f64>()
        .with_context(|| format!("`{token}` is not a number"))
}

fn strip_comment(line: &str) -> &str {
    line.split(';').next().unwrap_or("")
}

fn section_name(line: &str) -> Option<&str> {
    let inner = line.strip_prefix('[')?.strip_suffix(']')?;
    Some(inner.trim())
}

#[cfg(test)]
pub(crate) const METHANOL_ITP: &str = r#"; methanol_GMX.itp created by acpype (v: 2023.10.27) on Fri Oct 31 2026

[ atomtypes ]
;name   bond_type     mass     charge   ptype   sigma         epsilon       Amb
 c3       c3          0.00000  0.00000   A     3.39771e-01   4.51035e-01 ; 1.91  0.1078
 oh       oh          0.00000  0.00000   A     3.24287e-01   3.89112e-01 ; 1.82  0.0930
 h1       h1          0.00000  0.00000   A     2.42146e-01   8.70272e-02 ; 1.36  0.0208
 ho       ho          0.00000  0.00000   A     5.37925e-02   1.96648e-02 ; 0.30  0.0047

[ moleculetype ]
;name            nrexcl
 MOL             3

[ atoms ]
;   nr  type  resi  res  atom  cgnr     charge      mass       ; qtot   bond_type
     1   c3     1   MOL    C1    1     0.116700     12.01000 ; qtot 0.117
     2   oh     1   MOL    O1    2    -0.598800     16.00000 ; qtot -0.482
     3   h1     1   MOL    H1    3     0.028700      1.00800 ; qtot -0.453
     4   h1     1   MOL    H2    4     0.028700      1.00800 ; qtot -0.425
     5   h1     1   MOL    H3    5     0.028700      1.00800 ; qtot -0.396
     6   ho     1   MOL    H4    6     0.396000      1.00800 ; qtot 0.000

[ bonds ]
;   ai     aj funct   r             k
     1      2   1    1.4233e-01    2.5861e+05 ;     C1 - O1
     1      3   1    1.0970e-01    2.7665e+05 ;     C1 - H1
     1      4   1    1.0970e-01    2.7665e+05 ;     C1 - H2
     1      5   1    1.0970e-01    2.7665e+05 ;     C1 - H3
     2      6   1    9.7300e-02    3.0928e+05 ;     O1 - H4

[ pairs ]
;   ai     aj    funct
     3      6      1 ;     H1 - H4
     4      6      1 ;     H2 - H4
     5      6      1 ;     H3 - H4

[ angles ]
;   ai     aj     ak    funct   theta         cth
     1      2      6      1    1.0826e+02    3.9497e+02 ;     C1 - O1 - H4
     2      1      3      1    1.0991e+02    4.2886e+02 ;     O1 - C1 - H1
     2      1      4      1    1.0991e+02    4.2886e+02 ;     O1 - C1 - H2
     2      1      5      1    1.0991e+02    4.2886e+02 ;     O1 - C1 - H3
     3      1      4      1    1.0855e+02    3.2719e+02 ;     H1 - C1 - H2
     3      1      5      1    1.0855e+02    3.2719e+02 ;     H1 - C1 - H3
     4      1      5      1    1.0855e+02    3.2719e+02 ;     H2 - C1 - H3

[ dihedrals ] ; propers
; for gromacs 4.5 or higher, using funct 9
;    i      j      k      l   func   phase     kd      pn
     3      1      2      6      9     0.00   0.69733   3 ;     H1-  C1-  O1-  H4
     4      1      2      6      9     0.00   0.69733   3 ;     H2-  C1-  O1-  H4
     5      1      2      6      9     0.00   0.69733   3 ;     H3-  C1-  O1-  H4
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_an_acpype_itp() {
        let molecule = parse_itp(METHANOL_ITP, "acpype gaff2 bcc").unwrap();
        assert_eq!(molecule.name, "MOL");
        assert_eq!(molecule.nrexcl, 3);
        assert_eq!(molecule.atom_types.len(), 4);
        assert_eq!(molecule.atoms.len(), 6);
        assert_eq!(molecule.bonds.len(), 5);
        assert_eq!(molecule.pairs.len(), 3);
        assert_eq!(molecule.angles.len(), 7);
        assert_eq!(molecule.dihedrals.len(), 3);
        assert!(molecule.net_charge().abs() < 1.0e-6);
        let oxygen = &molecule.atoms[1];
        assert_eq!(oxygen.type_name, "oh");
        assert_eq!(oxygen.residue_name, "MOL");
        assert!((oxygen.mass_u - 16.0).abs() < 1.0e-6);
        let c3 = molecule.atom_type("c3").unwrap();
        assert!((c3.sigma_nm - 0.339771).abs() < 1.0e-6);
        assert_eq!(molecule.bonds[0].atoms, [1, 2]);
        assert_eq!(molecule.bonds[0].func, 1);
        assert_eq!(molecule.bonds[0].params.len(), 2);
        assert_eq!(molecule.dihedrals[0].func, 9);
        assert_eq!(molecule.dihedrals[0].params, [0.0, 0.69733, 3.0]);
        assert_eq!(molecule.type_counts()["h1"], 3);
    }

    #[test]
    fn rejects_an_atom_with_an_undefined_type() {
        let text = "[ atomtypes ]\nc3 c3 0 0 A 0.3 0.4\n[ atoms ]\n1 xx 1 MOL C1 1 0.0 12.0\n";
        let err = parse_itp(text, "test").unwrap_err().to_string();
        assert!(err.contains("`xx`"), "{err}");
    }

    #[test]
    fn rejects_a_bonded_term_outside_the_molecule() {
        let text = "[ atomtypes ]\nc3 c3 0 0 A 0.3 0.4\n[ atoms ]\n1 c3 1 MOL C1 1 0.0 12.0\n[ bonds ]\n1 2 1 0.1 1000\n";
        let err = parse_itp(text, "test").unwrap_err().to_string();
        assert!(err.contains("outside"), "{err}");
    }

    #[test]
    fn rejects_an_empty_topology() {
        assert!(parse_itp("; nothing here\n", "test").is_err());
    }
}
