//! Force-field-family nonbonded blocks for the generated `.mdp` (Appendix A).
//!
//! A biomolecular force field fixes how electrostatics and van der Waals are
//! treated, and the conventions must not be mixed (CHARMM's force-switch at
//! 1.2 nm with `DispCorr = no` versus AMBER/OPLS potential-shift at 1.0 nm with
//! `DispCorr = EnerPres`). This module renders the block for a chosen
//! [`ForceFieldFamily`]; the cut-off lengths come from the stage (so a user
//! override is honored), while the vdW modifier and dispersion correction are
//! family-fixed.
//!
//! The legacy plain-`cutoff` scheme (homogeneous Lennard-Jones / framework
//! systems) is rendered inline by [`super::input::render_mdp`] and is *not* routed
//! through here — its byte output is frozen by a stability test, and PME is a new
//! path for biomolecular systems rather than a rewrite of the old one.

use serde::{Deserialize, Serialize};

use crate::workflows::molecular_dynamics::ForceFieldFamily;

/// How a stage treats nonbonded interactions in the generated `.mdp`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum NonbondedScheme {
    /// Plain cut-off electrostatics with no force-field-specific vdW treatment.
    /// The historical default for homogeneous Lennard-Jones / framework systems;
    /// its rendered block is byte-stable.
    #[default]
    Cutoff,
    /// PME electrostatics with the family's conventional vdW treatment, for a
    /// biomolecular force field.
    ForceField(ForceFieldFamily),
}

/// Pad an `.mdp` key to the column the rest of the file aligns its `=` to, then
/// append the value. Matches the manual spacing in [`super::input::render_mdp`].
fn mdp_line(key: &str, value: &str) -> String {
    format!("{key:<25}= {value}\n")
}

/// Render the PME + vdW block for a biomolecular force-field family (Appendix A).
/// `rcoulomb`/`rvdw` are the stage's cut-offs (nm); the modifier and dispersion
/// correction are family-fixed. Does not emit `pbc` (the caller adds it), so the
/// output is the nonbonded block only.
pub fn force_field_block(family: ForceFieldFamily, rcoulomb_nm: f32, rvdw_nm: f32) -> String {
    let rc = format!("{rcoulomb_nm:.4}");
    let rv = format!("{rvdw_nm:.4}");
    let mut body = String::new();
    body.push_str(&mdp_line("nstlist", "10"));
    body.push_str(&mdp_line("cutoff-scheme", "Verlet"));
    body.push_str(&mdp_line("coulombtype", "PME"));

    match family {
        ForceFieldFamily::Charmm => {
            // Force-switched vdW out to 1.2 nm, no dispersion correction.
            body.push_str(&mdp_line("rcoulomb", &rc));
            body.push_str(&mdp_line("vdwtype", "cutoff"));
            body.push_str(&mdp_line("vdw-modifier", "force-switch"));
            body.push_str(&mdp_line("rvdw-switch", "1.0"));
            body.push_str(&mdp_line("rvdw", &rv));
            body.push_str(&mdp_line("rlist", &rc));
            body.push_str(&mdp_line("DispCorr", "no"));
        }
        ForceFieldFamily::Gromos => {
            // United-atom: potential is plain-shifted, dispersion-corrected, 1.4 nm.
            body.push_str(&mdp_line("rcoulomb", &rc));
            body.push_str(&mdp_line("rvdw", &rv));
            body.push_str(&mdp_line("DispCorr", "EnerPres"));
        }
        ForceFieldFamily::Amber | ForceFieldFamily::Opls | ForceFieldFamily::Other => {
            // Potential-shift vdW with a dispersion correction, 1.0 nm.
            body.push_str(&mdp_line("rcoulomb", &rc));
            body.push_str(&mdp_line("rvdw", &rv));
            body.push_str(&mdp_line("vdw-modifier", "potential-shift"));
            body.push_str(&mdp_line("DispCorr", "EnerPres"));
            if matches!(family, ForceFieldFamily::Amber) {
                body.push_str(&mdp_line("fourierspacing", "0.12000"));
            }
        }
    }
    body
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn amber_block_is_pme_potential_shift_with_dispersion_correction() {
        let block = force_field_block(ForceFieldFamily::Amber, 1.0, 1.0);
        assert!(block.contains("coulombtype              = PME"));
        assert!(block.contains("vdw-modifier             = potential-shift"));
        assert!(block.contains("DispCorr                 = EnerPres"));
        assert!(block.contains("fourierspacing           = 0.12000"));
        // AMBER must not carry CHARMM's force-switch convention.
        assert!(!block.contains("force-switch"));
    }

    #[test]
    fn charmm_block_is_force_switch_at_its_cutoff_no_dispersion() {
        let block = force_field_block(ForceFieldFamily::Charmm, 1.2, 1.2);
        assert!(block.contains("coulombtype              = PME"));
        assert!(block.contains("vdw-modifier             = force-switch"));
        assert!(block.contains("rvdw-switch              = 1.0"));
        assert!(block.contains("rvdw                     = 1.2000"));
        assert!(block.contains("DispCorr                 = no"));
        // CHARMM must not carry the AMBER/OPLS potential-shift convention.
        assert!(!block.contains("potential-shift"));
        assert!(!block.contains("EnerPres"));
    }

    #[test]
    fn cutoffs_come_from_the_stage() {
        // A user-overridden cut-off flows into the rendered block.
        let block = force_field_block(ForceFieldFamily::Amber, 1.1, 0.9);
        assert!(block.contains("rcoulomb                 = 1.1000"));
        assert!(block.contains("rvdw                     = 0.9000"));
    }
}
