use crate::domain::glycan::{Anomer, Linkage};

#[derive(Debug, Clone, Copy)]
pub struct TorsionPreference {
    pub phi: f32,
    pub psi: f32,
}

pub fn preferred_torsion(linkage: &Linkage) -> TorsionPreference {
    let alpha = linkage.anomer == Anomer::Alpha;
    match (alpha, linkage.parent_pos) {
        (true, 6) => TorsionPreference {
            phi: -65.0,
            psi: 180.0,
        },
        (false, 6) => TorsionPreference {
            phi: -75.0,
            psi: 180.0,
        },
        (true, 4) => TorsionPreference {
            phi: -65.0,
            psi: -40.0,
        },
        (false, 4) => TorsionPreference {
            phi: -75.0,
            psi: 130.0,
        },
        (true, 3) => TorsionPreference {
            phi: -65.0,
            psi: -25.0,
        },
        (false, 3) => TorsionPreference {
            phi: -75.0,
            psi: 120.0,
        },
        (true, 2) => TorsionPreference {
            phi: -60.0,
            psi: -50.0,
        },
        (false, 2) => TorsionPreference {
            phi: -70.0,
            psi: 110.0,
        },
        (true, _) => TorsionPreference {
            phi: -65.0,
            psi: -40.0,
        },
        (false, _) => TorsionPreference {
            phi: -75.0,
            psi: 130.0,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn linkage(anomer: Anomer, parent_pos: u8) -> Linkage {
        Linkage {
            anomer,
            child_pos: 1,
            parent_pos,
        }
    }

    #[test]
    fn beta_and_alpha_one_to_four_differ() {
        let beta = preferred_torsion(&linkage(Anomer::Beta, 4));
        let alpha = preferred_torsion(&linkage(Anomer::Alpha, 4));
        assert!((beta.psi - alpha.psi).abs() > 1.0);
    }

    #[test]
    fn one_to_six_arms_use_extended_psi() {
        let alpha = preferred_torsion(&linkage(Anomer::Alpha, 6));
        let beta = preferred_torsion(&linkage(Anomer::Beta, 6));
        assert_eq!(alpha.psi, 180.0);
        assert_eq!(beta.psi, 180.0);
    }
}
