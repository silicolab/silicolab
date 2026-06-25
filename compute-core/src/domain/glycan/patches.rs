pub const APPROXIMATE_LINKAGE_PATCH: bool = true;

pub const ETHER_BRIDGE_OXYGEN_TYPE: &str = "OC301";

pub const ETHER_BRIDGE_OXYGEN_CHARGE: f32 = -0.36;

pub const ANOMERIC_CARBON_DELTA: f32 = 0.16;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LinkagePatch {
    pub bridge_oxygen_type: &'static str,
    pub bridge_oxygen_charge: f32,
    pub anomeric_carbon_delta: f32,
    pub approximate: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeavingAtoms {
    pub donor_oxygen: String,
    pub donor_hydroxyl_hydrogen: String,
    pub acceptor_hydroxyl_hydrogen: String,
}

pub fn hexopyranose_linkage_patch() -> LinkagePatch {
    LinkagePatch {
        bridge_oxygen_type: ETHER_BRIDGE_OXYGEN_TYPE,
        bridge_oxygen_charge: ETHER_BRIDGE_OXYGEN_CHARGE,
        anomeric_carbon_delta: ANOMERIC_CARBON_DELTA,
        approximate: APPROXIMATE_LINKAGE_PATCH,
    }
}

pub fn leaving_atoms(child_pos: u8, parent_pos: u8) -> LeavingAtoms {
    LeavingAtoms {
        donor_oxygen: format!("O{child_pos}"),
        donor_hydroxyl_hydrogen: format!("HO{child_pos}"),
        acceptor_hydroxyl_hydrogen: format!("HO{parent_pos}"),
    }
}

pub fn anomeric_carbon_name(child_pos: u8) -> String {
    format!("C{child_pos}")
}

pub fn bridge_oxygen_name(parent_pos: u8) -> String {
    format!("O{parent_pos}")
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProteinChargeDelta {
    pub atom_name: &'static str,
    pub delta: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct JunctionPatch {
    pub anomeric_carbon_delta: f32,
    pub protein_deltas: Vec<ProteinChargeDelta>,
    pub approximate: bool,
}

pub const N_LINKED_ANOMERIC_CARBON_DELTA: f32 = 0.04;

pub const O_LINKED_ANOMERIC_CARBON_DELTA: f32 = 0.04;

pub fn n_linked_junction_patch() -> JunctionPatch {
    JunctionPatch {
        anomeric_carbon_delta: N_LINKED_ANOMERIC_CARBON_DELTA,
        protein_deltas: vec![
            ProteinChargeDelta {
                atom_name: "ND2",
                delta: 0.06,
            },
            ProteinChargeDelta {
                atom_name: "CG",
                delta: 0.05,
            },
            ProteinChargeDelta {
                atom_name: "HD21",
                delta: -0.04,
            },
            ProteinChargeDelta {
                atom_name: "HD22",
                delta: -0.04,
            },
        ],
        approximate: true,
    }
}

pub fn o_linked_junction_patch(anchor_oxygen: &str) -> JunctionPatch {
    let hydrogen = match anchor_oxygen {
        "OG" => "HG",
        "OG1" => "HG1",
        _ => "HG",
    };
    JunctionPatch {
        anomeric_carbon_delta: O_LINKED_ANOMERIC_CARBON_DELTA,
        protein_deltas: vec![
            ProteinChargeDelta {
                atom_name: leak_atom_name(anchor_oxygen),
                delta: -0.06,
            },
            ProteinChargeDelta {
                atom_name: hydrogen,
                delta: 0.06,
            },
        ],
        approximate: true,
    }
}

fn leak_atom_name(name: &str) -> &'static str {
    match name {
        "OG" => "OG",
        "OG1" => "OG1",
        _ => "OG",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leaving_atoms_follow_linkage_positions() {
        let leaving = leaving_atoms(1, 4);
        assert_eq!(leaving.donor_oxygen, "O1");
        assert_eq!(leaving.donor_hydroxyl_hydrogen, "HO1");
        assert_eq!(leaving.acceptor_hydroxyl_hydrogen, "HO4");
    }

    #[test]
    fn neu5ac_donor_is_anomeric_c2() {
        let leaving = leaving_atoms(2, 3);
        assert_eq!(leaving.donor_oxygen, "O2");
        assert_eq!(anomeric_carbon_name(2), "C2");
        assert_eq!(bridge_oxygen_name(3), "O3");
    }

    #[test]
    fn hexopyranose_patch_is_flagged_approximate() {
        let patch = hexopyranose_linkage_patch();
        assert!(patch.approximate);
        assert!(patch.bridge_oxygen_charge > -0.5 && patch.bridge_oxygen_charge < -0.2);
    }
}
