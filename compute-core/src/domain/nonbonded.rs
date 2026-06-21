//! Engine-neutral nonbonded (Lennard-Jones) parameters.
//!
//! This is project-global physical data, reusable across MD engines — it is
//! deliberately *not* part of any engine's invocation interface. Any MD backend
//! consumes the same numbers; backends differ only in how they serialize them.
//! Keeping the table here (in `domain`, the pure data layer) avoids prematurely
//! binding the chemistry to one engine.
//!
//! Units follow the `domain` convention of angstroms for length (matching
//! `Atom.position` and `UnitCell`); energies are in kJ/mol, the common MD
//! convention. Engine adapters convert to their own units (e.g. one using
//! nanometers divides sigma by 10).
//!
//! Only the noble gases are tabulated — the monatomic Lennard-Jones species
//! the MD System Builder can describe without a full force field. Extend this
//! table as more chemistry is supported.

use crate::domain::chemistry::normalized_symbol;

/// Engine-neutral Lennard-Jones description of a single atomic species.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LennardJones {
    /// Atomic number (proton count).
    pub atomic_number: u32,
    /// Atomic mass, unified atomic mass units (u).
    pub mass_u: f32,
    /// Lennard-Jones collision diameter sigma, angstroms.
    pub sigma_angstrom: f32,
    /// Lennard-Jones well depth epsilon, kJ/mol.
    pub epsilon_kj_mol: f32,
}

/// Lennard-Jones parameters for `element` (case-insensitive symbol), or `None`
/// if the species has no tabulated parameters yet.
pub fn lennard_jones(element: &str) -> Option<LennardJones> {
    Some(match normalized_symbol(element).as_str() {
        "He" => LennardJones {
            atomic_number: 2,
            mass_u: 4.0026,
            sigma_angstrom: 2.5560,
            epsilon_kj_mol: 0.08495,
        },
        "Ne" => LennardJones {
            atomic_number: 10,
            mass_u: 20.1797,
            sigma_angstrom: 2.8200,
            epsilon_kj_mol: 0.28500,
        },
        "Ar" => LennardJones {
            atomic_number: 18,
            mass_u: 39.948,
            sigma_angstrom: 3.4050,
            epsilon_kj_mol: 0.99600,
        },
        "Kr" => LennardJones {
            atomic_number: 36,
            mass_u: 83.798,
            sigma_angstrom: 3.6360,
            epsilon_kj_mol: 1.38500,
        },
        "Xe" => LennardJones {
            atomic_number: 54,
            mass_u: 131.293,
            sigma_angstrom: 3.9750,
            epsilon_kj_mol: 1.91000,
        },
        _ => return None,
    })
}

/// Elements with tabulated Lennard-Jones parameters.
pub fn supported_elements() -> &'static [&'static str] {
    &["He", "Ne", "Ar", "Kr", "Xe"]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn argon_matches_the_bundled_example_in_nm() {
        let ar = lennard_jones("Ar").expect("argon tabulated");
        // MD engines use nm; sigma_angstrom / 10 must equal the reference value.
        assert!((ar.sigma_angstrom * 0.1 - 0.34050).abs() < 1e-6);
        assert!((ar.epsilon_kj_mol - 0.99600).abs() < 1e-6);
        assert_eq!(ar.atomic_number, 18);
    }

    #[test]
    fn lookup_is_case_insensitive() {
        assert_eq!(lennard_jones("ar"), lennard_jones("Ar"));
        assert_eq!(lennard_jones("AR"), lennard_jones("Ar"));
    }

    #[test]
    fn unsupported_element_returns_none() {
        assert!(lennard_jones("C").is_none());
    }

    #[test]
    fn every_supported_element_resolves() {
        for element in supported_elements() {
            assert!(lennard_jones(element).is_some(), "missing {element}");
        }
    }
}
