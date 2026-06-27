use super::*;
use crate::domain::glycan::dictionary::lookup;

#[test]
fn builds_a_chair_template_for_glucose() {
    let mono = lookup("aGlc").unwrap().mono;
    let template = ring_template(mono).unwrap();
    assert!(template.atoms.iter().any(|a| a.name == "C1"));
    assert!(template.atoms.iter().any(|a| a.name == "O5"));
    let ring: Vec<&str> = template
        .atoms
        .iter()
        .filter(|a| ["C1", "C2", "C3", "C4", "C5", "O5"].contains(&a.name.as_str()))
        .map(|a| a.name.as_str())
        .collect();
    assert_eq!(ring.len(), 6);
}

#[test]
fn declares_acceptor_sites_for_ring_hydroxyls() {
    let mono = lookup("Gal").unwrap().mono;
    let template = ring_template(mono).unwrap();
    let positions: Vec<u8> = template.acceptor_sites.iter().map(|(p, _)| *p).collect();
    for pos in [2u8, 3, 4, 6] {
        assert!(positions.contains(&pos), "missing acceptor O{pos}");
    }
}

#[test]
fn donor_site_binds_the_anomeric_carbon() {
    let mono = lookup("Neu5Ac").unwrap().mono;
    let template = ring_template(mono).unwrap();
    let binding = template.donor_site.binding_atom;
    assert!(
        template
            .acceptor_sites
            .iter()
            .all(|(_, site)| site.coordination_position.norm() > 0.0)
    );
    assert!(template.atoms.get(binding).is_some());
}

#[test]
fn sialic_ring_closes_on_c2_and_o6() {
    let mono = lookup("Neu5Ac").unwrap().mono;
    let template = ring_template(mono).unwrap();
    let name = |i: usize| template.atoms[i].name.as_str();
    let has_bond = |x: &str, y: &str| {
        template
            .bonds
            .iter()
            .any(|b| (name(b.a) == x && name(b.b) == y) || (name(b.a) == y && name(b.b) == x))
    };
    assert!(has_bond("C2", "O6"));
    assert!(has_bond("C6", "O6"));
    assert!(has_bond("C2", "C3"));
    assert!(has_bond("C2", "C1"));
    assert!(!has_bond("C1", "O5"));
    let binding = template.donor_site.binding_atom;
    assert_eq!(name(binding), "C2");
}

#[test]
fn ring_bonds_close_the_pyranose() {
    let mono = lookup("aMan").unwrap().mono;
    let template = ring_template(mono).unwrap();
    let name = |i: usize| template.atoms[i].name.as_str();
    let has_bond = |x: &str, y: &str| {
        template
            .bonds
            .iter()
            .any(|b| (name(b.a) == x && name(b.b) == y) || (name(b.a) == y && name(b.b) == x))
    };
    assert!(has_bond("C5", "O5"));
    assert!(has_bond("C1", "O5"));
    assert!(has_bond("C1", "C2"));
}

fn template_has_bond(template: &RingTemplate, x: &str, y: &str) -> bool {
    template.bonds.iter().any(|b| {
        let (a, c) = (
            template.atoms[b.a].name.as_str(),
            template.atoms[b.b].name.as_str(),
        );
        (a == x && c == y) || (a == y && c == x)
    })
}

/// The acetamido/glycolyl nitrogen must attach to its own ring carbon — C2 on
/// the hexosamines, C5 on the sialic acids — and to no other. (Regression
/// guard: the roster once emitted a spurious second N–ring bond on hexosamines.)
#[test]
fn acetamido_nitrogen_bonds_only_its_own_ring_carbon() {
    for token in ["GlcNAc", "GalNAc", "ManNAc"] {
        let template = ring_template(lookup(token).unwrap().mono).unwrap();
        assert!(
            template_has_bond(&template, "C2", "N"),
            "{token}: N–C2 missing"
        );
        assert!(
            !template_has_bond(&template, "C5", "N"),
            "{token}: spurious N–C5 bond"
        );
    }
    for token in ["Neu5Ac", "Neu5Gc"] {
        let template = ring_template(lookup(token).unwrap().mono).unwrap();
        assert!(
            template_has_bond(&template, "C5", "N"),
            "{token}: N–C5 missing"
        );
        assert!(
            !template_has_bond(&template, "C2", "N"),
            "{token}: spurious N–C2 bond"
        );
    }
}

/// Deterministic placement is now the sole arbiter of stereochemistry, so guard
/// it geometrically — bond lengths and clash distances cannot tell a sugar from
/// its stereoisomer. The signed tetrahedral volume at a stereocentre flips when
/// a substituent crosses between the ring faces, so anomers and epimers (across
/// both D and L sugars) must come out with opposite sign.
#[test]
fn anomers_and_epimers_have_opposite_3d_chirality() {
    let chirality = |token: &str, center: &str, a: &str, b: &str, c: &str| -> f32 {
        let template = ring_template(lookup(token).unwrap().mono).unwrap();
        let pos = |name: &str| {
            template
                .atoms
                .iter()
                .find(|at| at.name == name)
                .unwrap_or_else(|| panic!("{token} missing {name}"))
                .position
        };
        let o = pos(center);
        (pos(a) - o).dot(&(pos(b) - o).cross(&(pos(c) - o)))
    };

    // α- vs β-D-Glc: the anomeric O1 sits on opposite faces at C1.
    let alpha = chirality("aGlc", "C1", "C2", "O5", "O1");
    let beta = chirality("Glc", "C1", "C2", "O5", "O1");
    assert!(
        alpha * beta < 0.0,
        "α/β-Glc anomeric chirality not opposite: {alpha} vs {beta}"
    );

    // Glc vs Gal are C4 epimers; Glc vs Man are C2 epimers.
    let glc_c4 = chirality("Glc", "C4", "C3", "C5", "O4");
    let gal_c4 = chirality("Gal", "C4", "C3", "C5", "O4");
    assert!(
        glc_c4 * gal_c4 < 0.0,
        "Glc/Gal C4 chirality not opposite: {glc_c4} vs {gal_c4}"
    );
    let glc_c2 = chirality("Glc", "C2", "C1", "C3", "O2");
    let man_c2 = chirality("Man", "C2", "C1", "C3", "O2");
    assert!(
        glc_c2 * man_c2 < 0.0,
        "Glc/Man C2 chirality not opposite: {glc_c2} vs {man_c2}"
    );

    // L-fucose is 6-deoxy-L-galactose. In this fixed 4C1 frame the L inversion
    // places its galacto C4 hydroxyl on the equatorial face — opposite to
    // D-Gal's axial C4. Without the Fuc/C4 epimer entry, Fuc's C4 would instead
    // land axial like D-Gal, so this pins that fix.
    let fuc_c4 = chirality("Fuc", "C4", "C3", "C5", "O4");
    assert!(
        fuc_c4 * gal_c4 < 0.0,
        "L-Fuc C4 face must invert vs D-Gal in the fixed chair: {fuc_c4} vs {gal_c4}"
    );
    // α-L-Fuc's anomeric O1 must invert relative to α-D-Glc — the anomeric
    // centre follows the absolute configuration like every other ring carbon.
    let fuc_c1 = chirality("Fuc", "C1", "C2", "O5", "O1");
    assert!(
        fuc_c1 * alpha < 0.0,
        "α-L-Fuc anomeric chirality must invert vs α-D-Glc: {fuc_c1} vs {alpha}"
    );
}
