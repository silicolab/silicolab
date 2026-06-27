//! Generation of GROMACS input files (`.gro`, `.mdp`) from SilicoLab structures.
//!
//! GROMACS stages (energy minimization, NVT, NPT, production) differ only in
//! the `.mdp` parameters fed to `gmx grompp`, so this module exposes one
//! [`MdpSettings`] type that covers them all. Presets such as
//! [`MdpSettings::energy_minimization`] populate the fields for a specific
//! stage; new stages are added as additional constructors.
//!
//! Generates `.mdp` for homogeneous Lennard-Jones systems: plain cut-off
//! electrostatics, no bond constraints, single `System` thermostat group.

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};

use crate::domain::Structure;
use crate::engines::gromacs::nonbonded::{NonbondedScheme, force_field_block};

/// Conversion factor angstroms -> nanometers (GROMACS uses nm).
const ANGSTROM_TO_NM: f32 = 0.1;

/// Which time-integration scheme GROMACS should run for this stage. Maps
/// directly to the `integrator =` line in the generated `.mdp`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Integrator {
    /// Steepest-descent energy minimization (`integrator = steep`).
    SteepestDescent,
    /// Leap-frog molecular dynamics (`integrator = md`).
    Leapfrog,
}

impl Integrator {
    pub fn mdp_token(self) -> &'static str {
        match self {
            Self::SteepestDescent => "steep",
            Self::Leapfrog => "md",
        }
    }

    pub fn is_minimization(self) -> bool {
        matches!(self, Self::SteepestDescent)
    }
}

/// Which bonds GROMACS converts to holonomic constraints. `None` on
/// [`MdpSettings`] leaves them flexible (`constraints = none`); constraining
/// hydrogen bonds (`h-bonds`) lets MD use a 2 fs timestep.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConstraintKind {
    HBonds,
    AllBonds,
}

impl ConstraintKind {
    pub fn mdp_token(self) -> &'static str {
        match self {
            Self::HBonds => "h-bonds",
            Self::AllBonds => "all-bonds",
        }
    }
}

/// Constraint solver algorithm (`constraint-algorithm =`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConstraintAlgorithm {
    Lincs,
    Shake,
}

impl ConstraintAlgorithm {
    pub fn mdp_token(self) -> &'static str {
        match self {
            Self::Lincs => "lincs",
            Self::Shake => "shake",
        }
    }
}

/// Thermostat algorithm (`tcoupl =`), rendered only when temperature coupling is
/// active. Defaults to velocity rescaling, the robust standard choice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Thermostat {
    VRescale,
    NoseHoover,
    /// Weak coupling — equilibration only.
    Berendsen,
}

impl Thermostat {
    pub fn mdp_token(self) -> &'static str {
        match self {
            Self::VRescale => "V-rescale",
            Self::NoseHoover => "Nose-Hoover",
            Self::Berendsen => "Berendsen",
        }
    }
}

/// Barostat algorithm (`pcoupl =`). Stochastic cell rescaling is the modern
/// default and needs GROMACS >= 2021; older engines fall back to the
/// Berendsen-equilibration / Parrinello-Rahman-production pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Barostat {
    CRescale,
    ParrinelloRahman,
    /// Weak coupling — equilibration only.
    Berendsen,
}

impl Barostat {
    pub fn mdp_token(self) -> &'static str {
        match self {
            Self::CRescale => "C-rescale",
            Self::ParrinelloRahman => "Parrinello-Rahman",
            Self::Berendsen => "Berendsen",
        }
    }
}

/// A simulated-annealing temperature ramp applied to every coupling group:
/// `(time_ps, temperature_k)` control points.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Annealing {
    pub points: Vec<(f32, f32)>,
}

/// Temperature-coupling configuration. `tc_grps`, `tau_t` and `ref_t` are
/// parallel: one entry per coupling group. The thermostat is velocity-rescaling
/// (`V-rescale`), the standard robust choice.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemperatureCoupling {
    pub tc_grps: Vec<String>,
    pub tau_t: Vec<f32>,
    pub ref_t: Vec<f32>,
}

impl TemperatureCoupling {
    /// Couple the whole system as one group at `ref_t` kelvin — the right
    /// arrangement for a homogeneous system with no solute/solvent split.
    /// `System` is the default index group GROMACS always defines.
    pub fn whole_system(ref_t: f32) -> Self {
        Self {
            tc_grps: vec!["System".to_string()],
            tau_t: vec![0.1],
            ref_t: vec![ref_t],
        }
    }
}

/// Pressure-coupling configuration. `ref_p`/`compressibility` are parallel
/// vectors: one entry for isotropic, two for semi-isotropic (xy, z), more for
/// anisotropic.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PressureCoupling {
    pub barostat: Barostat,
    pub pcoupltype: String,
    pub tau_p: f32,
    pub ref_p: Vec<f32>,
    pub compressibility: Vec<f32>,
}

impl PressureCoupling {
    /// Isotropic coupling to 1 bar with the modern stochastic cell-rescale
    /// barostat — the standard NPT/production setting.
    pub fn isotropic() -> Self {
        Self {
            barostat: Barostat::CRescale,
            pcoupltype: "isotropic".to_string(),
            tau_p: 2.0,
            ref_p: vec![1.0],
            compressibility: vec![4.5e-5],
        }
    }

    /// Semi-isotropic coupling to 1 bar (membrane systems: in-plane xy and
    /// normal z couple independently).
    pub fn semi_isotropic() -> Self {
        Self {
            pcoupltype: "semiisotropic".to_string(),
            ref_p: vec![1.0, 1.0],
            compressibility: vec![4.5e-5, 4.5e-5],
            ..Self::isotropic()
        }
    }
}

/// Initial-velocity generation (`gen_vel =`). Present only for the first MD
/// stage (NVT); later stages continue from the checkpoint instead.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct VelocityGen {
    pub gen_temp: f32,
    /// Random seed; `-1` lets GROMACS pick one.
    pub gen_seed: i64,
}

/// Trajectory/energy output frequencies (steps between writes).
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct OutputFrequency {
    pub nstxout: u32,
    pub nstvout: u32,
    pub nstenergy: u32,
    pub nstlog: u32,
    /// Compressed (`.xtc`) trajectory write interval; `0` disables it.
    pub nstxout_compressed: u32,
}

impl OutputFrequency {
    /// Equilibration: log energy regularly, no full-precision trajectory.
    pub fn equilibration() -> Self {
        Self {
            nstxout: 0,
            nstvout: 0,
            nstenergy: 500,
            nstlog: 500,
            nstxout_compressed: 0,
        }
    }

    /// Production: write a compressed `.xtc` trajectory for analysis.
    pub fn production() -> Self {
        Self {
            nstxout: 0,
            nstvout: 0,
            nstenergy: 5_000,
            nstlog: 5_000,
            nstxout_compressed: 5_000,
        }
    }
}

/// `.mdp` parameters shared across all GROMACS stages. Stage-specific fields
/// (e.g. `emtol` for minimization, `dt` for MD) are kept on the same struct;
/// [`render_mdp`] picks the right subset depending on `integrator`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MdpSettings {
    pub integrator: Integrator,
    pub nsteps: u64,
    pub timestep_ps: f32,
    pub coulomb_cutoff_nm: f32,
    pub vdw_cutoff_nm: f32,
    /// Force tolerance for minimization (kJ/mol/nm). Only used when
    /// `integrator == SteepestDescent`.
    pub emtol: f32,
    /// Initial step size for minimization (nm). Only used when
    /// `integrator == SteepestDescent`.
    pub emstep: f32,
    /// `continuation = yes/no` — rendered for MD stages only. NPT/production
    /// continue a prior run; the first NVT stage does not.
    pub continuation: bool,
    /// Temperature coupling; `None` leaves the thermostat off.
    pub temperature_coupling: Option<TemperatureCoupling>,
    /// Pressure coupling; `None` renders `pcoupl = no` for MD stages.
    pub pressure_coupling: Option<PressureCoupling>,
    /// Initial velocity generation; `None` renders `gen_vel = no` for MD.
    pub velocity_generation: Option<VelocityGen>,
    /// Output write frequencies; `None` falls back to GROMACS defaults.
    pub output: Option<OutputFrequency>,
    /// Bonds to constrain; `None` renders `constraints = none`. MD stages
    /// default to `h-bonds` (for a 2 fs step); minimization leaves it `None`.
    pub constraints: Option<ConstraintKind>,
    /// Constraint solver used when `constraints` is set.
    pub constraint_algorithm: ConstraintAlgorithm,
    /// Emit `periodic-molecules = yes`. Required when a molecule is bonded across
    /// the periodic boundary (a flexible periodic framework such as a nanosheet),
    /// so grompp does not try to make the molecule whole.
    pub periodic_molecules: bool,
    /// Freeze a group of atoms in place (all three dimensions). Used to hold a
    /// rigid framework fixed while the surrounding system evolves. The named
    /// group must exist in the index file passed to grompp.
    pub freeze: Option<FreezeGroup>,
    /// Electrostatics/vdW treatment. Defaults to the legacy plain cut-off (whose
    /// rendered block is byte-stable); a biomolecular run sets PME plus the
    /// force-field nonbonded block.
    pub nonbonded: NonbondedScheme,
    /// `define =` preprocessor flags (e.g. `-DPOSRES` to switch on position
    /// restraints). `None` emits no `define` line.
    pub define: Option<String>,
    /// Thermostat algorithm; rendered only when `temperature_coupling` is set.
    pub thermostat: Thermostat,
    /// Simulated-annealing ramp; `None` emits no annealing block.
    pub annealing: Option<Annealing>,
    /// Raw `key = value` lines appended verbatim last (engine passthrough). May
    /// introduce any directive — including ones with no dedicated field — and,
    /// being last, overrides earlier ones.
    pub raw_lines: Vec<(String, String)>,
}

/// A set of atoms frozen in place during a run. Only full (3D) freezing is
/// supported, which is the only mode the Verlet cutoff scheme honors reliably.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FreezeGroup {
    /// Index group name (must appear in the `.ndx` file given to `grompp -n`).
    pub group: String,
}

impl MdpSettings {
    /// Defaults for a steepest-descent energy minimization stage.
    pub fn energy_minimization() -> Self {
        Self {
            integrator: Integrator::SteepestDescent,
            nsteps: 5_000,
            timestep_ps: 0.0,
            coulomb_cutoff_nm: 1.0,
            vdw_cutoff_nm: 1.0,
            emtol: 1_000.0,
            emstep: 0.01,
            continuation: false,
            temperature_coupling: None,
            pressure_coupling: None,
            velocity_generation: None,
            output: None,
            constraints: None,
            constraint_algorithm: ConstraintAlgorithm::Lincs,
            periodic_molecules: false,
            freeze: None,
            nonbonded: NonbondedScheme::Cutoff,
            define: None,
            thermostat: Thermostat::VRescale,
            annealing: None,
            raw_lines: Vec::new(),
        }
    }

    /// NVT (constant volume/temperature) equilibration with freshly generated
    /// velocities and a single whole-`System` thermostat. 100 ps at a 2 fs step,
    /// which `h-bonds` constraints make stable.
    pub fn nvt(ref_temp: f32) -> Self {
        Self {
            integrator: Integrator::Leapfrog,
            nsteps: 50_000,
            timestep_ps: 0.002,
            continuation: false,
            temperature_coupling: Some(TemperatureCoupling::whole_system(ref_temp)),
            pressure_coupling: None,
            velocity_generation: Some(VelocityGen {
                gen_temp: ref_temp,
                gen_seed: -1,
            }),
            output: Some(OutputFrequency::equilibration()),
            constraints: Some(ConstraintKind::HBonds),
            ..Self::energy_minimization()
        }
    }

    /// NPT (constant pressure/temperature) equilibration: continues from the NVT
    /// checkpoint and adds an isotropic barostat.
    pub fn npt(ref_temp: f32) -> Self {
        Self {
            continuation: true,
            velocity_generation: None,
            pressure_coupling: Some(PressureCoupling::isotropic()),
            ..Self::nvt(ref_temp)
        }
    }

    /// Production MD: full temperature and pressure coupling, and a compressed
    /// trajectory for analysis.
    pub fn production(nsteps: u64, ref_temp: f32) -> Self {
        Self {
            nsteps,
            continuation: true,
            velocity_generation: None,
            pressure_coupling: Some(PressureCoupling::isotropic()),
            output: Some(OutputFrequency::production()),
            ..Self::nvt(ref_temp)
        }
    }
}

impl Default for MdpSettings {
    fn default() -> Self {
        Self::energy_minimization()
    }
}

/// Serialize the active structure as a GROMACS coordinate (`.gro`) file.
pub fn to_gro(structure: &Structure, title: &str) -> Result<String> {
    let cell = structure
        .cell
        .as_ref()
        .ok_or_else(|| anyhow!("GROMACS coordinate files need a simulation box vector"))?;
    if structure.atoms.is_empty() {
        return Err(anyhow!("cannot write a GROMACS .gro file with zero atoms"));
    }

    let header = title.lines().next().unwrap_or(title).trim();
    let mut output = String::new();
    output.push_str(if header.is_empty() {
        "silicolab-structure"
    } else {
        header
    });
    output.push('\n');
    output.push_str(&format!("{:>5}\n", structure.atoms.len()));

    for (index, atom) in structure.atoms.iter().enumerate() {
        let serial = (index + 1) as u32 % 100_000;
        let residue_id = 1u32;
        let residue_name = "MOL";
        let atom_name = atom_name_for(&atom.element, index + 1);

        output.push_str(&format!(
            "{:>5}{:<5}{:>5}{:>5}{:>8.3}{:>8.3}{:>8.3}\n",
            residue_id,
            residue_name,
            atom_name,
            serial,
            atom.position.x * ANGSTROM_TO_NM,
            atom.position.y * ANGSTROM_TO_NM,
            atom.position.z * ANGSTROM_TO_NM,
        ));
    }

    output.push_str(&format_box_vectors(cell.vectors));
    Ok(output)
}

/// Emit a `.mdp` parameter file appropriate for the configured stage.
///
/// Every MD-only directive (thermostat, barostat, velocity generation, output
/// frequencies) is emitted only for non-minimization stages or when the
/// relevant field is populated.
pub fn render_mdp(settings: &MdpSettings) -> String {
    let mut body = String::new();
    body.push_str("; SilicoLab-generated GROMACS run parameters\n");
    if let Some(define) = &settings.define {
        // Preprocessor flags such as `-DPOSRES`, which switch on the position
        // restraints whose `#ifdef POSRES` block lives in the topology's
        // posre.itp. Omitted entirely when unset (production drops restraints).
        body.push_str(&format!("define                   = {define}\n"));
    }

    body.push_str(&format!(
        "integrator               = {}\n",
        settings.integrator.mdp_token()
    ));
    body.push_str(&format!("nsteps                   = {}\n", settings.nsteps));

    if settings.integrator.is_minimization() {
        body.push_str(&format!(
            "emtol                    = {:.3}\n",
            settings.emtol
        ));
        body.push_str(&format!(
            "emstep                   = {:.5}\n",
            settings.emstep
        ));
    } else {
        body.push_str(&format!(
            "dt                       = {:.5}\n",
            settings.timestep_ps
        ));
    }

    match settings.nonbonded {
        // Legacy plain cut-off (homogeneous LJ / framework). Kept byte-for-byte
        // identical to the historical output — a stability test depends on it.
        NonbondedScheme::Cutoff => {
            body.push_str("nstlist                  = 10\n");
            body.push_str("cutoff-scheme            = Verlet\n");
            body.push_str("ns_type                  = grid\n");
            body.push_str("coulombtype              = cutoff\n");
            body.push_str(&format!(
                "rcoulomb                 = {:.4}\n",
                settings.coulomb_cutoff_nm
            ));
            body.push_str(&format!(
                "rvdw                     = {:.4}\n",
                settings.vdw_cutoff_nm
            ));
        }
        // PME + the force field's nonbonded block (biomolecular systems).
        NonbondedScheme::ForceField(family) => {
            body.push_str(&force_field_block(
                family,
                settings.coulomb_cutoff_nm,
                settings.vdw_cutoff_nm,
            ));
        }
    }
    body.push_str("pbc                      = xyz\n");
    if settings.periodic_molecules {
        // Required when a molecule is bonded across the periodic boundary (a
        // flexible periodic framework): keep it as-is rather than trying to make
        // it whole.
        body.push_str("periodic-molecules       = yes\n");
    }
    if let Some(freeze) = &settings.freeze {
        // Hold a rigid framework fixed in all three dimensions. Full freezing is
        // the only mode the Verlet scheme honors reliably.
        body.push_str(&format!("freezegrps               = {}\n", freeze.group));
        body.push_str("freezedim                = Y Y Y\n");
    }
    match settings.constraints {
        Some(kind) => {
            body.push_str(&format!(
                "constraints              = {}\n",
                kind.mdp_token()
            ));
            body.push_str(&format!(
                "constraint-algorithm     = {}\n",
                settings.constraint_algorithm.mdp_token()
            ));
        }
        None => body.push_str("constraints              = none\n"),
    }

    if !settings.integrator.is_minimization() {
        render_md_coupling(&mut body, settings);
    }

    // Raw passthrough, appended verbatim last so it can override anything above
    // (and introduce keys with no dedicated field).
    for (key, value) in &settings.raw_lines {
        body.push_str(&format!("{key:<25}= {value}\n"));
    }

    body
}

/// Append the MD-only coupling, velocity-generation, continuation, and output
/// directives. Split out to keep [`render_mdp`] readable.
fn render_md_coupling(body: &mut String, settings: &MdpSettings) {
    if let Some(tc) = &settings.temperature_coupling {
        body.push_str(&format!(
            "tcoupl                   = {}\n",
            settings.thermostat.mdp_token()
        ));
        body.push_str(&format!(
            "tc-grps                  = {}\n",
            tc.tc_grps.join(" ")
        ));
        body.push_str(&format!(
            "tau-t                    = {}\n",
            join_floats(&tc.tau_t)
        ));
        body.push_str(&format!(
            "ref-t                    = {}\n",
            join_floats(&tc.ref_t)
        ));
    }

    if let Some(pc) = &settings.pressure_coupling {
        body.push_str(&format!(
            "pcoupl                   = {}\n",
            pc.barostat.mdp_token()
        ));
        body.push_str(&format!("pcoupltype               = {}\n", pc.pcoupltype));
        body.push_str(&format!("tau-p                    = {}\n", pc.tau_p));
        body.push_str(&format!(
            "ref-p                    = {}\n",
            join_floats(&pc.ref_p)
        ));
        body.push_str(&format!(
            "compressibility          = {}\n",
            join_floats(&pc.compressibility)
        ));
    } else {
        body.push_str("pcoupl                   = no\n");
    }

    if let Some(vg) = &settings.velocity_generation {
        body.push_str("gen_vel                  = yes\n");
        body.push_str(&format!("gen_temp                 = {}\n", vg.gen_temp));
        body.push_str(&format!("gen_seed                 = {}\n", vg.gen_seed));
    } else {
        body.push_str("gen_vel                  = no\n");
    }

    body.push_str(&format!(
        "continuation             = {}\n",
        if settings.continuation { "yes" } else { "no" }
    ));

    if let Some(out) = &settings.output {
        body.push_str(&format!("nstxout                  = {}\n", out.nstxout));
        body.push_str(&format!("nstvout                  = {}\n", out.nstvout));
        body.push_str(&format!("nstenergy                = {}\n", out.nstenergy));
        body.push_str(&format!("nstlog                   = {}\n", out.nstlog));
        body.push_str(&format!(
            "nstxout-compressed       = {}\n",
            out.nstxout_compressed
        ));
    }

    if let Some(annealing) = &settings.annealing {
        render_annealing(body, settings, annealing);
    }
}

/// Append a simulated-annealing block. GROMACS expects one entry per temperature
/// coupling group; the same ramp is applied to every group, with each group's
/// control points concatenated in `annealing-time`/`annealing-temp`.
fn render_annealing(body: &mut String, settings: &MdpSettings, annealing: &Annealing) {
    let groups = settings
        .temperature_coupling
        .as_ref()
        .map_or(1, |tc| tc.tc_grps.len())
        .max(1);
    let npoints = annealing.points.len();
    let times: Vec<String> = (0..groups)
        .flat_map(|_| annealing.points.iter().map(|(time, _)| time.to_string()))
        .collect();
    let temps: Vec<String> = (0..groups)
        .flat_map(|_| annealing.points.iter().map(|(_, temp)| temp.to_string()))
        .collect();
    body.push_str(&format!(
        "annealing                = {}\n",
        vec!["single"; groups].join(" ")
    ));
    body.push_str(&format!(
        "annealing-npoints        = {}\n",
        vec![npoints.to_string(); groups].join(" ")
    ));
    body.push_str(&format!("annealing-time           = {}\n", times.join(" ")));
    body.push_str(&format!("annealing-temp           = {}\n", temps.join(" ")));
}

/// Render a list of floats space-separated using their compact `Display` form
/// (`0.1`, `300`), matching how the GROMACS examples write coupling vectors.
fn join_floats(values: &[f32]) -> String {
    values
        .iter()
        .map(|v| v.to_string())
        .collect::<Vec<_>>()
        .join(" ")
}

fn format_box_vectors(vectors: [nalgebra::Vector3<f32>; 3]) -> String {
    let nm = |v: f32| v * ANGSTROM_TO_NM;
    let [v1, v2, v3] = reduce_box(vectors);

    let off_diag_tolerance = 1.0e-5_f32;
    let triclinic = v1.y.abs() > off_diag_tolerance
        || v1.z.abs() > off_diag_tolerance
        || v2.x.abs() > off_diag_tolerance
        || v2.z.abs() > off_diag_tolerance
        || v3.x.abs() > off_diag_tolerance
        || v3.y.abs() > off_diag_tolerance;

    if triclinic {
        format!(
            "{:>10.5}{:>10.5}{:>10.5}{:>10.5}{:>10.5}{:>10.5}{:>10.5}{:>10.5}{:>10.5}\n",
            nm(v1.x),
            nm(v2.y),
            nm(v3.z),
            nm(v1.y),
            nm(v1.z),
            nm(v2.x),
            nm(v2.z),
            nm(v3.x),
            nm(v3.y),
        )
    } else {
        format!("{:>10.5}{:>10.5}{:>10.5}\n", nm(v1.x), nm(v2.y), nm(v3.z))
    }
}

/// Reduce a triclinic cell to the form GROMACS requires: a lower-triangular box
/// whose off-diagonal elements are no larger than half the corresponding
/// diagonal (`|v2x| ≤ v1x/2`, `|v3x| ≤ v1x/2`, `|v3y| ≤ v2y/2`). The lattice is
/// unchanged — only the choice of representative vectors. A cell already in
/// range (the hexagonal nanosheet cell sits exactly at the half boundary, which
/// GROMACS accepts) is returned untouched; an over-skewed cell is brought in.
fn reduce_box([v1, mut v2, mut v3]: [nalgebra::Vector3<f32>; 3]) -> [nalgebra::Vector3<f32>; 3] {
    // Shift `hi` by integer multiples of `lo` until component `comp` lies in
    // (-lo[comp]/2, +lo[comp]/2]. Strict comparisons leave an exact half in
    // place (so the canonical hexagonal/dodecahedral form is preserved).
    fn bring_in_range(hi: &mut nalgebra::Vector3<f32>, lo: nalgebra::Vector3<f32>, comp: usize) {
        if lo[comp].abs() < 1.0e-6 {
            return;
        }
        let half = 0.5 * lo[comp];
        let mut guard = 0;
        while hi[comp] > half && guard < 1_000 {
            *hi -= lo;
            guard += 1;
        }
        while hi[comp] < -half && guard < 1_000 {
            *hi += lo;
            guard += 1;
        }
    }

    bring_in_range(&mut v3, v2, 1);
    bring_in_range(&mut v3, v1, 0);
    bring_in_range(&mut v2, v1, 0);
    [v1, v2, v3]
}

fn atom_name_for(element: &str, serial: usize) -> String {
    let element = element.trim();
    if element.is_empty() {
        return format!("X{}", serial % 1_000);
    }
    let candidate = format!("{element}{}", serial % 1_000);
    let trimmed: String = candidate.chars().take(5).collect();
    if trimmed.is_empty() {
        "X".to_string()
    } else {
        trimmed
    }
}

#[cfg(test)]
mod tests;
