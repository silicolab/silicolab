#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComponentSource {
    /// Index into CORE_COMPONENTS
    BuiltinCore(usize),
    /// Index into LINKER_COMPONENTS
    BuiltinLinker(usize),
    /// Index into FUNCTIONAL_GROUP_COMPONENTS
    BuiltinFunctionalGroup(usize),
    /// Custom component from user-provided mol2 string
    Custom(usize),
}

impl ComponentSource {
    /// Construct a builtin core component reference (reserved for CLI/scripting).
    #[allow(dead_code)]
    pub fn builtin_core(idx: usize) -> Self {
        Self::BuiltinCore(idx)
    }

    /// Construct a builtin linker component reference (reserved for CLI/scripting).
    #[allow(dead_code)]
    pub fn builtin_linker(idx: usize) -> Self {
        Self::BuiltinLinker(idx)
    }

    /// Construct a builtin functional group component reference (reserved for CLI/scripting).
    #[allow(dead_code)]
    pub fn builtin_functional_group(idx: usize) -> Self {
        Self::BuiltinFunctionalGroup(idx)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkId {
    HoneycombVertexVertex,
}

impl NetworkId {
    pub fn label(self) -> &'static str {
        match self {
            Self::HoneycombVertexVertex => "HCB (T3 core + T3 core)",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StackingMode {
    C3Orientational,
}

impl StackingMode {
    pub fn layer_period(self) -> u32 {
        match self {
            Self::C3Orientational => 3,
        }
    }

    pub fn layer_azimuth_degrees(self, layer_index: u32) -> f32 {
        match self {
            Self::C3Orientational => (layer_index % 3) as f32 * 120.0,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ReticularBuildSpec {
    pub name: String,
    pub network: NetworkId,
    pub primary: ComponentSource,
    pub secondary: ComponentSource,
    pub linkers: Vec<ComponentSource>,
    pub linker_direction: LinkerDirection,
    pub functionalization_enabled: bool,
    pub functionalizations: Vec<FunctionalizationRule>,
    pub custom_components: Vec<String>,
    pub stacking: StackingMode,
    pub modulate_primary_orientation: bool,
    pub modulate_secondary_orientation: bool,
    pub layer_spacing: f32,
    pub supercell: [u32; 3],
}

impl ReticularBuildSpec {
    pub fn stacking_period(&self) -> u32 {
        if self.modulate_primary_orientation || self.modulate_secondary_orientation {
            self.stacking.layer_period()
        } else {
            1
        }
    }

    pub fn primary_layer_azimuth_degrees(&self, layer_index: u32) -> f32 {
        if self.modulate_primary_orientation {
            self.stacking.layer_azimuth_degrees(layer_index)
        } else {
            0.0
        }
    }

    pub fn secondary_layer_azimuth_degrees(&self, layer_index: u32) -> f32 {
        if self.modulate_secondary_orientation {
            self.stacking.layer_azimuth_degrees(layer_index)
        } else {
            0.0
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkerDirection {
    PrimaryToSecondary,
    SecondaryToPrimary,
}

impl LinkerDirection {
    pub fn label(self) -> &'static str {
        match self {
            Self::PrimaryToSecondary => "Core A -> Core B",
            Self::SecondaryToPrimary => "Core B -> Core A",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CoreSlot {
    Primary,
    Secondary,
    Linker(usize),
}

impl CoreSlot {
    pub fn label(self) -> String {
        match self {
            Self::Primary => "Core A".to_string(),
            Self::Secondary => "Core B".to_string(),
            Self::Linker(index) => format!("Linker {}", index + 1),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FunctionalizationRule {
    pub slot: CoreSlot,
    pub atom_index: usize,
    pub group: Option<ComponentSource>,
}

impl Default for ReticularBuildSpec {
    fn default() -> Self {
        Self {
            name: "Structure".to_string(),
            network: NetworkId::HoneycombVertexVertex,
            primary: ComponentSource::BuiltinCore(0),
            secondary: ComponentSource::BuiltinCore(0),
            linkers: Vec::new(),
            linker_direction: LinkerDirection::PrimaryToSecondary,
            functionalization_enabled: false,
            functionalizations: Vec::new(),
            custom_components: Vec::new(),
            stacking: StackingMode::C3Orientational,
            modulate_primary_orientation: false,
            modulate_secondary_orientation: false,
            layer_spacing: 3.6,
            supercell: [2, 2, 2],
        }
    }
}
