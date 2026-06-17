/// One molecule in a Build Disordered System launch: which workspace entry to
/// copy and how much of it. `count` drives [`DisorderAmount::Count`];
/// `amount_value` is the density (g/cm³) or concentration (mol/L) used for the
/// other amount modes.
#[derive(Debug, Clone)]
pub struct DisorderComponentDraft {
    pub entry_id: u64,
    pub count: u32,
    pub amount_value: f32,
}

impl Default for DisorderComponentDraft {
    fn default() -> Self {
        Self {
            entry_id: 0,
            count: 100,
            amount_value: 1.0,
        }
    }
}

/// The geometric region shape a disordered system is packed into.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DisorderRegionKind {
    #[default]
    Box,
    Sphere,
    Cylinder,
}

/// How a component's amount is specified.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DisorderAmount {
    /// A literal number of copies.
    #[default]
    Count,
    /// A target mass density in g/cm³ (count derived from the region volume).
    DensityGCm3,
    /// A target molar concentration in mol/L.
    ConcentrationMolar,
}

impl DisorderAmount {
    pub fn label(self) -> &'static str {
        match self {
            Self::Count => "Copies",
            Self::DensityGCm3 => "Density (g/cm³)",
            Self::ConcentrationMolar => "Concentration (mol/L)",
        }
    }
}

/// Draft for a Build Disordered System launch — packing of one or
/// more molecules into a box, sphere, or cylinder. Mirrors the role of
/// [`MdRunPrompt`] for the disorder task; consumed by `start_pending_disorder`.
#[derive(Debug, Clone)]
pub struct DisorderedSystemPrompt {
    /// Title of the new combined entry the build produces.
    pub output_name: String,
    /// The molecule types and their amounts (at least one to launch).
    pub components: Vec<DisorderComponentDraft>,
    /// How the per-component amount is read.
    pub amount_mode: DisorderAmount,
    pub region_kind: DisorderRegionKind,
    pub box_lengths: [f32; 3],
    pub sphere_radius: f32,
    pub cyl_radius: f32,
    pub cyl_length: f32,
    /// Pack outside the region (carve a void) rather than inside it.
    pub sense_outside: bool,
    /// Minimum inter-molecular atom spacing (Å).
    pub tolerance_angstrom: f32,
    pub seed: u64,
    /// An existing entry to pack around (its atoms stay fixed).
    pub obstacle_entry_id: Option<u64>,
    pub max_restarts: u32,
    pub max_steps: u32,
    /// Stamp the region as the result's simulation cell (box regions only).
    pub set_cell_from_region: bool,
    /// Pack periodically with no clashes across box edges (box regions only).
    pub periodic: bool,
    pub show_advanced: bool,
}

impl Default for DisorderedSystemPrompt {
    fn default() -> Self {
        Self {
            output_name: "Disordered system".to_string(),
            components: Vec::new(),
            amount_mode: DisorderAmount::Count,
            region_kind: DisorderRegionKind::Box,
            box_lengths: [40.0, 40.0, 40.0],
            sphere_radius: 20.0,
            cyl_radius: 15.0,
            cyl_length: 40.0,
            sense_outside: false,
            tolerance_angstrom: 2.0,
            seed: 1,
            obstacle_entry_id: None,
            max_restarts: 20,
            max_steps: 2000,
            set_cell_from_region: true,
            periodic: false,
            show_advanced: false,
        }
    }
}
