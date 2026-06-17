#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoordinateOptimizationScope {
    AllAtoms,
    SelectedAtoms,
}

/// Per-atom drawing style, applied to a selection of atoms.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum AtomStyle {
    /// Polymer-backbone ribbon. Only standard amino-acid residues actually
    /// render as cartoon; other atoms styled this way are not drawn.
    Cartoon,
    /// Not drawn at all.
    Hidden,
    /// A small flat disc per atom. Cheapest; ideal for bulk solvent and ions.
    Point,
    /// Bonds as thin lines only; atoms carry no marker. Ideal for bulk
    /// solvent — pure lines, no dots.
    Wireframe,
    /// Bonds as cylinders, no atom spheres.
    Stick,
    /// Cylinders plus small atom spheres.
    #[default]
    BallAndStick,
    /// Full van der Waals spheres, no bonds.
    Sphere,
}

impl AtomStyle {
    pub fn all() -> &'static [Self] {
        &[
            Self::Cartoon,
            Self::BallAndStick,
            Self::Stick,
            Self::Wireframe,
            Self::Sphere,
            Self::Point,
            Self::Hidden,
        ]
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Cartoon => "Cartoon",
            Self::Hidden => "Hidden",
            Self::Point => "Dots",
            Self::Wireframe => "Wireframe",
            Self::Stick => "Stick",
            Self::BallAndStick => "Ball-and-stick",
            Self::Sphere => "Sphere (VdW)",
        }
    }

    /// Stable string token for persistence and the console.
    pub fn token(self) -> &'static str {
        match self {
            Self::Cartoon => "cartoon",
            Self::Hidden => "hidden",
            Self::Point => "dots",
            Self::Wireframe => "wireframe",
            Self::Stick => "stick",
            Self::BallAndStick => "ball-stick",
            Self::Sphere => "sphere",
        }
    }

    pub fn from_token(token: &str) -> Option<Self> {
        Some(match token {
            "cartoon" => Self::Cartoon,
            "hidden" | "hide" => Self::Hidden,
            "dots" | "point" | "points" => Self::Point,
            "wireframe" | "line" | "lines" => Self::Wireframe,
            "stick" | "licorice" => Self::Stick,
            "ball-stick" | "ball_and_stick" => Self::BallAndStick,
            "sphere" | "spheres" | "vdw" => Self::Sphere,
            _ => return None,
        })
    }

    /// Whether visible atoms in this style are drawn as a flat point disc. Only
    /// `Point` (Dots) draws a disc; `Wireframe` shows bonds as lines with no
    /// per-atom marker.
    pub fn draws_point(self) -> bool {
        matches!(self, Self::Point)
    }

    /// True for styles whose per-atom geometry is heavy enough — tessellated
    /// spheres and/or bond cylinders — that very large selections must be
    /// downgraded to points to stay within the GPU buffer. The cheap line/dot
    /// styles draw little and are exempt.
    pub fn is_heavy(self) -> bool {
        matches!(self, Self::Sphere | Self::BallAndStick | Self::Stick)
    }

    /// Whether bonds touching an atom of this style are drawn as solid
    /// cylinders.
    pub fn draws_stick_bonds(self) -> bool {
        matches!(self, Self::Stick | Self::BallAndStick)
    }

    /// Whether bonds touching an atom of this style are drawn as thin lines.
    pub fn draws_line_bonds(self) -> bool {
        matches!(self, Self::Wireframe)
    }
}
