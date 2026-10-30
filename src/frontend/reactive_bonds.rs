//! Per-frame bond perception for reactive trajectories (quantum / semi-empirical
//! MD), so playback shows bonds forming and breaking instead of the fixed
//! topology of the base structure.
//!
//! The whole trajectory is analysed once, up front: a bond's state depends on
//! its history (hysteresis keeps a vibrating bond from flickering), and knowing
//! the future lets a bond that is about to break be drawn as breaking. Scrubbing
//! then just indexes the precomputed frames.

use eframe::egui::Color32;

use crate::domain::{Bond, BondType, Trajectory, chemistry::element_style};

/// A bond forms below this multiple of the summed covalent radii — the same
/// cutoff static bond perception uses, so frame 0 matches the entry's bonds.
const FORM_FACTOR: f32 = 1.25;
/// A formed bond only counts as broken beyond this multiple. The gap between the
/// two factors is the hysteresis band a thermally vibrating bond lives in.
const BREAK_FACTOR: f32 = 1.45;
/// H–H is the exception: an H₂ activated on a metal (a σ-complex) stretches to
/// ~1.0 Å and is still H₂, so it breaks only past ~1.1 Å.
const HH_FORM_FACTOR: f32 = 1.30;
const HH_BREAK_FACTOR: f32 = 1.75;
/// Bonding is judged on distances averaged over this window, so a bond's own
/// vibration (an H–H stretch period is ~8 fs) never reads as breaking.
const SMOOTHING_WINDOW_PS: f32 = 0.025;
const EVENT_WINDOW: usize = 12;
/// Above this many atom pairs × frames the analysis is skipped (a solvated
/// classical MD run neither needs nor can afford it).
const MAX_PAIR_FRAMES: usize = 60_000_000;

const FORMING_COLOR: Color32 = Color32::from_rgb(64, 214, 255);
const BREAKING_COLOR: Color32 = Color32::from_rgb(255, 120, 48);

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OverlayBond {
    pub a: usize,
    pub b: usize,
    pub radius_scale: f32,
    /// Color drawn over the bond's atom-colored halves, with its strength in alpha.
    pub tint: Color32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BondEvent {
    pub frame: usize,
    pub a: usize,
    pub b: usize,
    pub formed: bool,
}

#[derive(Debug, Clone, Default)]
struct FrameBonds {
    plain: Vec<(usize, usize)>,
    overlay: Vec<OverlayBond>,
}

#[derive(Debug, Clone, Default)]
pub struct ReactiveBonds {
    frames: Vec<FrameBonds>,
    events: Vec<BondEvent>,
    key: Option<(usize, usize)>,
    headlines: Vec<BondEvent>,
}

impl ReactiveBonds {
    /// Analyse `trajectory` over the given element symbols. `None` when the
    /// trajectory is too large to analyse.
    pub fn analyse(trajectory: &Trajectory, elements: &[String]) -> Option<Self> {
        let natoms = trajectory.natoms().min(elements.len());
        let frames = trajectory.frame_count();
        let pairs = natoms * natoms.saturating_sub(1) / 2;
        if natoms == 0 || frames == 0 || pairs.saturating_mul(frames) > MAX_PAIR_FRAMES {
            return None;
        }
        let radii: Vec<f32> = elements[..natoms]
            .iter()
            .map(|symbol| element_style(symbol).covalent_radius)
            .collect();
        let hydrogen: Vec<bool> = elements[..natoms]
            .iter()
            .map(|symbol| symbol.eq_ignore_ascii_case("H") || symbol.eq_ignore_ascii_case("D"))
            .collect();
        let metal: Vec<bool> = elements[..natoms]
            .iter()
            .map(|symbol| is_metal(symbol))
            .collect();
        let index_pairs: Vec<(usize, usize)> = (0..natoms)
            .flat_map(|a| ((a + 1)..natoms).map(move |b| (a, b)))
            .collect();

        let mut ratio = vec![vec![0.0_f32; pairs]; frames];
        for (frame, row) in ratio.iter_mut().enumerate() {
            for (pair, &(a, b)) in index_pairs.iter().enumerate() {
                row[pair] = nalgebra::distance(
                    &trajectory.position(frame, a),
                    &trajectory.position(frame, b),
                ) / (radii[a] + radii[b]).max(0.1);
            }
        }
        let dt = if frames > 1 {
            (trajectory.time(1) - trajectory.time(0)).abs()
        } else {
            0.0
        };
        let half = if dt > 0.0 {
            ((SMOOTHING_WINDOW_PS / dt / 2.0).round() as usize).min(25)
        } else {
            0
        };
        let smoothed: Vec<Vec<f32>> = (0..frames)
            .map(|frame| {
                let lo = frame.saturating_sub(half);
                let hi = (frame + half).min(frames - 1);
                (0..pairs)
                    .map(|pair| {
                        (lo..=hi).map(|f| ratio[f][pair]).sum::<f32>() / (hi - lo + 1) as f32
                    })
                    .collect()
            })
            .collect();

        let factors: Vec<(f32, f32)> = index_pairs
            .iter()
            .map(|&(a, b)| {
                if hydrogen[a] && hydrogen[b] {
                    (HH_FORM_FACTOR, HH_BREAK_FACTOR)
                } else {
                    (FORM_FACTOR, BREAK_FACTOR)
                }
            })
            .collect();
        let mut bonded = vec![vec![false; pairs]; frames];
        for frame in 0..frames {
            for pair in 0..pairs {
                let (form, brk) = factors[pair];
                let r = smoothed[frame][pair];
                // Bonds present at the start are judged as existing bonds: an H₂
                // that is already activated on the metal is still H₂.
                let before = frame == 0 || bonded[frame - 1][pair];
                bonded[frame][pair] = if before { r <= brk } else { r <= form };
            }
        }
        let mut events = Vec::new();
        for (pair, &(a, b)) in index_pairs.iter().enumerate() {
            for frame in 1..frames {
                if bonded[frame][pair] != bonded[frame - 1][pair] {
                    events.push(BondEvent {
                        frame,
                        a,
                        b,
                        formed: bonded[frame][pair],
                    });
                }
            }
        }
        events.sort_by_key(|event| (event.frame, event.a, event.b));

        let mut out = Vec::with_capacity(frames);
        for frame in 0..frames {
            let mut bonds = FrameBonds::default();
            for (pair, &(a, b)) in index_pairs.iter().enumerate() {
                if !bonded[frame][pair] {
                    continue;
                }
                // A fluxional metal cluster rearranges its own bonds all the
                // time; only bonds to the adsorbate get the forming/breaking cue.
                if metal[a] && metal[b] {
                    bonds.plain.push((a, b));
                    continue;
                }
                let since_formed = (1..=EVENT_WINDOW.min(frame))
                    .find(|&back| !bonded[frame - back][pair])
                    .map(|back| back - 1);
                let until_broken = (1..=EVENT_WINDOW)
                    .take_while(|&ahead| frame + ahead < frames)
                    .find(|&ahead| !bonded[frame + ahead][pair]);
                let (form, brk) = factors[pair];
                let stretch = ((smoothed[frame][pair] - form) / (brk - form)).clamp(0.0, 1.0);
                if let Some(ahead) = until_broken {
                    let closeness = 1.0 - (ahead as f32 - 1.0) / EVENT_WINDOW as f32;
                    bonds.overlay.push(OverlayBond {
                        a,
                        b,
                        radius_scale: 1.0 - 0.65 * closeness,
                        tint: with_alpha(BREAKING_COLOR, 0.35 + 0.65 * closeness),
                    });
                } else if let Some(age) = since_formed.filter(|_| frame > 0) {
                    let growth = age as f32 / EVENT_WINDOW as f32;
                    bonds.overlay.push(OverlayBond {
                        a,
                        b,
                        radius_scale: 0.35 + 0.65 * growth,
                        tint: with_alpha(FORMING_COLOR, 1.0 - 0.8 * growth),
                    });
                } else if stretch > 0.0 {
                    bonds.overlay.push(OverlayBond {
                        a,
                        b,
                        radius_scale: 1.0 - 0.35 * stretch,
                        tint: with_alpha(BREAKING_COLOR, 0.5 * stretch),
                    });
                } else {
                    bonds.plain.push((a, b));
                }
            }
            out.push(bonds);
        }
        let key = events
            .iter()
            .find(|event| !event.formed && !metal[event.a] && !metal[event.b])
            .or_else(|| {
                events
                    .iter()
                    .find(|event| !event.formed && metal[event.a] != metal[event.b])
            })
            .map(|event| (event.a, event.b));
        let mut adsorbed = vec![false; natoms];
        let mut headlines = Vec::new();
        for event in &events {
            if key == Some((event.a, event.b)) {
                headlines.push(*event);
            } else if event.formed && metal[event.a] != metal[event.b] {
                let adsorbate = if metal[event.a] { event.b } else { event.a };
                if !adsorbed[adsorbate]
                    && !bonded_to_metal_at_start(&bonded, &index_pairs, &metal, adsorbate)
                {
                    adsorbed[adsorbate] = true;
                    headlines.push(*event);
                }
            }
        }
        Some(Self {
            frames: out,
            events,
            key,
            headlines,
        })
    }

    pub fn plain_bonds(&self, frame: usize) -> Vec<Bond> {
        self.frames
            .get(frame)
            .map(|bonds| {
                bonds
                    .plain
                    .iter()
                    .map(|&(a, b)| Bond::with_type(a, b, BondType::Single))
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn overlay(&self, frame: usize) -> &[OverlayBond] {
        self.frames
            .get(frame)
            .map(|bonds| bonds.overlay.as_slice())
            .unwrap_or(&[])
    }

    pub fn events(&self) -> &[BondEvent] {
        &self.events
    }

    /// The bond the trajectory is about: the first to break between two
    /// non-metals (H–H when H₂ dissociates), else the first adsorbate–metal bond
    /// to break. Metal–metal rearrangements never qualify.
    pub fn key_bond(&self) -> Option<(usize, usize)> {
        self.key
    }

    /// Events worth announcing: each change of the key bond, and each adsorbate
    /// atom's first bond to the metal.
    pub fn headlines(&self) -> &[BondEvent] {
        &self.headlines
    }
}

fn bonded_to_metal_at_start(
    bonded: &[Vec<bool>],
    pairs: &[(usize, usize)],
    metal: &[bool],
    atom: usize,
) -> bool {
    pairs.iter().enumerate().any(|(pair, &(a, b))| {
        ((a == atom && metal[b]) || (b == atom && metal[a])) && bonded[0][pair]
    })
}

const NONMETALS: &[&str] = &[
    "H", "D", "He", "B", "C", "N", "O", "F", "Ne", "Si", "P", "S", "Cl", "Ar", "As", "Se", "Br",
    "Kr", "Te", "I", "Xe", "At", "Rn",
];

pub(crate) fn is_metal(symbol: &str) -> bool {
    !NONMETALS
        .iter()
        .any(|nonmetal| nonmetal.eq_ignore_ascii_case(symbol))
}

fn with_alpha(color: Color32, alpha: f32) -> Color32 {
    Color32::from_rgba_unmultiplied(
        color.r(),
        color.g(),
        color.b(),
        (alpha.clamp(0.0, 1.0) * 255.0).round() as u8,
    )
}

#[cfg(test)]
mod tests {
    use super::ReactiveBonds;
    use crate::domain::Trajectory;

    fn stretching_h2() -> (Trajectory, Vec<String>) {
        let frames = 200;
        let mut coords = Vec::new();
        for frame in 0..frames {
            let d = 0.74 + (2.0 - 0.74) * frame as f32 / (frames - 1) as f32;
            coords.extend_from_slice(&[0.0, 0.0, 0.0, 0.0, 0.0, d]);
        }
        let times = (0..frames).map(|f| f as f32).collect();
        (
            Trajectory::from_parts(2, coords, times, 1, frames),
            vec!["H".to_string(), "H".to_string()],
        )
    }

    #[test]
    fn a_stretched_bond_breaks_once_past_the_hysteresis_band() {
        let (trajectory, elements) = stretching_h2();
        let bonds = ReactiveBonds::analyse(&trajectory, &elements).expect("analysed");
        assert_eq!(bonds.events().len(), 1);
        let event = bonds.events()[0];
        assert_eq!((event.a, event.b, event.formed), (0, 1, false));
        assert_eq!(bonds.key_bond(), Some((0, 1)));
        assert_eq!(bonds.headlines(), &[event]);
        // 0.62 Å summed radii × 1.75 ≈ 1.085 Å (H–H's wider band).
        let d = 0.74 + 1.26 * event.frame as f32 / 199.0;
        assert!(d > 1.084 && d < 1.10, "broke at {d}");
        assert_eq!(bonds.plain_bonds(0).len(), 1);
        assert_eq!(bonds.overlay(event.frame - 1).len(), 1);
        assert!(bonds.plain_bonds(event.frame).is_empty());
        assert!(bonds.overlay(event.frame).is_empty());
    }

    #[test]
    fn hysteresis_keeps_a_vibrating_bond_steady() {
        // Oscillates between 0.69 and 0.83 Å: inside the band, never re-perceived.
        let frames = 50;
        let mut coords = Vec::new();
        for frame in 0..frames {
            let d = 0.76 + 0.07 * (frame as f32 * 0.7).sin();
            coords.extend_from_slice(&[0.0, 0.0, 0.0, d, 0.0, 0.0]);
        }
        let times = (0..frames).map(|f| f as f32).collect();
        let trajectory = Trajectory::from_parts(2, coords, times, 1, frames);
        let bonds =
            ReactiveBonds::analyse(&trajectory, &["H".to_string(), "H".to_string()]).unwrap();
        assert!(bonds.events().is_empty());
    }
}
