//! Parametrized description of a 2D material nanosheet.
//!
//! Rather than enumerate individual materials, each sheet is described by a
//! *family* generator plus its composition parameters. The same `Honeycomb`
//! generator produces graphene (C/C), hexagonal boron nitride (B/N) and any
//! other binary honeycomb; the same `Tmd` generator produces every
//! transition-metal dichalcogenide MX2; the `CarbonNitride` generator produces
//! both polymorphs of graphitic carbon nitride. Named presets are simply
//! parameter bundles that the UI can drop into an otherwise editable spec.

/// In-plane trigonal-prismatic vs octahedral stacking of the two chalcogen
/// planes in a transition-metal dichalcogenide.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TmdPolytype {
    /// 1H/2H: both chalcogen planes eclipsed over the same hollow
    /// (trigonal-prismatic metal coordination, e.g. semiconducting MoS2).
    H,
    /// 1T: chalcogen planes staggered over opposite hollows
    /// (octahedral metal coordination).
    T,
}

impl TmdPolytype {
    pub fn label(self) -> &'static str {
        match self {
            Self::H => "1H (trigonal prismatic)",
            Self::T => "1T (octahedral)",
        }
    }
}

/// Polyatomic node tiling the honeycomb in graphitic carbon nitride.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CarbonNitrideNode {
    /// s-triazine (C3N3) rings bridged by N — exact C3N4 stoichiometry.
    Triazine,
    /// tri-s-triazine (heptazine, C6N7) units bridged by N.
    Heptazine,
}

impl CarbonNitrideNode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Triazine => "Triazine (C3N3)",
            Self::Heptazine => "Heptazine (C6N7)",
        }
    }

    /// Self-consistent in-plane lattice constant for the ideal node geometry.
    pub fn ideal_lattice_a(self) -> f32 {
        match self {
            // (ring circumradius + bridge bond) * sqrt(3)
            Self::Triazine => (RING_BOND + BRIDGE_BOND) * SQRT3,
            // (apex radius + bridge bond) * sqrt(3); apex radius = 2 ring bonds
            Self::Heptazine => (2.0 * RING_BOND + BRIDGE_BOND) * SQRT3,
        }
    }
}

/// Aromatic C–N bond inside a triazine/heptazine ring (Å).
pub const RING_BOND: f32 = 1.34;
/// C–N bond from a node carbon out to a bridging nitrogen (Å).
pub const BRIDGE_BOND: f32 = 1.40;
const SQRT3: f32 = 1.732_050_8;

#[derive(Debug, Clone, PartialEq)]
pub struct HoneycombParams {
    /// Element on the first honeycomb sublattice.
    pub element_a: String,
    /// Element on the second honeycomb sublattice (equal to `element_a` for graphene).
    pub element_b: String,
    /// In-plane lattice constant a (Å); equals the nearest-neighbour bond × √3.
    pub lattice_a: f32,
    /// Vertical offset between the two sublattices (Å); 0 for a flat sheet,
    /// non-zero for buckled honeycombs such as silicene.
    pub buckling: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TmdParams {
    /// Transition metal M.
    pub metal: String,
    /// Chalcogen X (S, Se, Te).
    pub chalcogen: String,
    /// In-plane lattice constant a (Å).
    pub lattice_a: f32,
    /// Vertical separation between the top and bottom chalcogen planes (Å).
    pub chalcogen_separation: f32,
    pub polytype: TmdPolytype,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CarbonNitrideParams {
    pub node: CarbonNitrideNode,
    /// In-plane lattice constant a (Å); spacing between node centres × √3.
    pub lattice_a: f32,
}

/// The lattice family a sheet belongs to, independent of its composition.
/// Drives the two-level "pick a type, then a preset" selection in the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SheetFamily {
    Honeycomb,
    Tmd,
    CarbonNitride,
}

impl SheetFamily {
    pub fn label(self) -> &'static str {
        match self {
            Self::Honeycomb => "Honeycomb (A/B)",
            Self::Tmd => "Transition-metal dichalcogenide (TMD, MX2)",
            Self::CarbonNitride => "Graphitic carbon nitride",
        }
    }

    pub fn all() -> &'static [SheetFamily] {
        &[Self::Honeycomb, Self::Tmd, Self::CarbonNitride]
    }

    /// The composition a freshly selected family starts from.
    pub fn default_kind(self) -> SheetKind {
        match self {
            Self::Honeycomb => SheetKind::Honeycomb(HoneycombParams::graphene()),
            Self::Tmd => SheetKind::Tmd(TmdParams::mos2()),
            Self::CarbonNitride => SheetKind::CarbonNitride(CarbonNitrideParams::triazine()),
        }
    }
}

/// The lattice family plus its composition parameters.
#[derive(Debug, Clone, PartialEq)]
pub enum SheetKind {
    Honeycomb(HoneycombParams),
    Tmd(TmdParams),
    CarbonNitride(CarbonNitrideParams),
}

impl SheetKind {
    pub fn family(&self) -> SheetFamily {
        match self {
            Self::Honeycomb(_) => SheetFamily::Honeycomb,
            Self::Tmd(_) => SheetFamily::Tmd,
            Self::CarbonNitride(_) => SheetFamily::CarbonNitride,
        }
    }
}

impl HoneycombParams {
    pub fn graphene() -> Self {
        Self {
            element_a: "C".to_string(),
            element_b: "C".to_string(),
            lattice_a: 2.46,
            buckling: 0.0,
        }
    }

    pub fn boron_nitride() -> Self {
        Self {
            element_a: "B".to_string(),
            element_b: "N".to_string(),
            lattice_a: 2.504,
            buckling: 0.0,
        }
    }

    pub fn silicene() -> Self {
        Self {
            element_a: "Si".to_string(),
            element_b: "Si".to_string(),
            lattice_a: 3.86,
            buckling: 0.44,
        }
    }
}

impl TmdParams {
    fn new(metal: &str, chalcogen: &str, lattice_a: f32, chalcogen_separation: f32) -> Self {
        Self {
            metal: metal.to_string(),
            chalcogen: chalcogen.to_string(),
            lattice_a,
            chalcogen_separation,
            polytype: TmdPolytype::H,
        }
    }

    pub fn mos2() -> Self {
        Self::new("Mo", "S", 3.16, 3.13)
    }
    pub fn ws2() -> Self {
        Self::new("W", "S", 3.153, 3.145)
    }
    pub fn mose2() -> Self {
        Self::new("Mo", "Se", 3.29, 3.34)
    }
    pub fn wse2() -> Self {
        Self::new("W", "Se", 3.282, 3.36)
    }
    pub fn mote2() -> Self {
        Self::new("Mo", "Te", 3.52, 3.60)
    }
}

impl CarbonNitrideParams {
    pub fn triazine() -> Self {
        Self {
            node: CarbonNitrideNode::Triazine,
            lattice_a: CarbonNitrideNode::Triazine.ideal_lattice_a(),
        }
    }

    pub fn heptazine() -> Self {
        Self {
            node: CarbonNitrideNode::Heptazine,
            lattice_a: CarbonNitrideNode::Heptazine.ideal_lattice_a(),
        }
    }
}

/// Whole nanosheet build request.
#[derive(Debug, Clone, PartialEq)]
pub struct NanosheetSpec {
    pub name: String,
    pub kind: SheetKind,
    /// Vacuum gap between a sheet surface and its next periodic image along c (Å).
    /// Reduce toward ~3.3 Å to model a bulk layered (graphitic) crystal.
    pub interlayer_spacing: f32,
    /// Lattice repeats applied after the primitive cell is built.
    pub supercell: [u32; 3],
}

impl Default for NanosheetSpec {
    fn default() -> Self {
        Self {
            name: "Nanosheet".to_string(),
            kind: SheetKind::Honeycomb(HoneycombParams::graphene()),
            interlayer_spacing: 12.0,
            supercell: [4, 4, 1],
        }
    }
}

/// Named composition presets that populate an editable [`SheetKind`].
pub fn presets() -> Vec<(&'static str, SheetKind)> {
    vec![
        (
            "Graphene",
            SheetKind::Honeycomb(HoneycombParams::graphene()),
        ),
        (
            "Hexagonal boron nitride",
            SheetKind::Honeycomb(HoneycombParams::boron_nitride()),
        ),
        (
            "Silicene",
            SheetKind::Honeycomb(HoneycombParams::silicene()),
        ),
        ("MoS2 (1H)", SheetKind::Tmd(TmdParams::mos2())),
        ("WS2 (1H)", SheetKind::Tmd(TmdParams::ws2())),
        ("MoSe2 (1H)", SheetKind::Tmd(TmdParams::mose2())),
        ("WSe2 (1H)", SheetKind::Tmd(TmdParams::wse2())),
        ("MoTe2 (1H)", SheetKind::Tmd(TmdParams::mote2())),
        (
            "g-C3N4 (triazine)",
            SheetKind::CarbonNitride(CarbonNitrideParams::triazine()),
        ),
        (
            "g-C3N4 (heptazine)",
            SheetKind::CarbonNitride(CarbonNitrideParams::heptazine()),
        ),
    ]
}
