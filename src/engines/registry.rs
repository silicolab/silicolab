//! Discovery of external computational chemistry engines.
//!
//! The registry probes a small set of well-known executables at startup,
//! records which engines are available, and honors per-engine launch
//! overrides persisted in `AppConfig::engine_overrides`. The built-in UFF
//! optimizer is exposed here too so that downstream code can treat all
//! engines uniformly.
//!
//! ## Launch model
//!
//! An engine is invoked through an [`EngineLaunch`]: a `program` plus an
//! optional `command_prefix`. Native engines have an empty prefix and run
//! directly. Engines that live behind a launcher (the canonical case being
//! GROMACS inside WSL) set a prefix such as `["wsl.exe", "-e"]`, so the
//! effective command becomes `wsl.exe -e <program> <args...>`.
//!
//! A command prefix covers WSL, containers, and wrapper scripts without a
//! full transport abstraction.

use std::{collections::HashMap, path::PathBuf, time::Duration};

use serde::{Deserialize, Serialize};

use crate::engines::process::{self, ProcessConfig};

/// Stable identifier used everywhere a specific engine is referenced.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EngineId(pub &'static str);

impl EngineId {
    pub const UFF: Self = Self("uff");
    pub const CHEMX: Self = Self("chemx");
    pub const GROMACS: Self = Self("gromacs");

    pub fn as_str(self) -> &'static str {
        self.0
    }
}

/// How to launch an external engine: a program plus an optional command
/// prefix. Native = empty prefix; WSL = `["wsl.exe", "-e"]`; a container or
/// wrapper script is just a different prefix.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EngineLaunch {
    /// Leading command tokens, e.g. `["wsl.exe", "-e"]`. Empty for native.
    #[serde(default)]
    pub command_prefix: Vec<String>,
    /// The engine executable, resolved on the *target* environment. For WSL
    /// this is a Linux path like `/usr/local/gromacs/bin/gmx`; for native it
    /// is a Windows path or a bare name found on PATH.
    pub program: String,
}

impl EngineLaunch {
    /// A native launch with no prefix.
    pub fn native(program: impl Into<String>) -> Self {
        Self {
            command_prefix: Vec::new(),
            program: program.into(),
        }
    }

    /// True when there is no usable program configured.
    pub fn is_empty(&self) -> bool {
        self.program.trim().is_empty()
    }

    /// Human-readable rendering of the effective command, for settings UI.
    pub fn display_command(&self) -> String {
        if self.command_prefix.is_empty() {
            self.program.clone()
        } else {
            format!("{} {}", self.command_prefix.join(" "), self.program)
        }
    }

    /// Build a [`ProcessConfig`] that runs this engine with `engine_args` in
    /// `working_dir`. The prefix's first token becomes the spawned executable;
    /// remaining prefix tokens, the program, and the engine args follow.
    pub fn to_process_config(
        &self,
        working_dir: impl Into<PathBuf>,
        engine_args: impl IntoIterator<Item = String>,
        timeout: Option<Duration>,
    ) -> ProcessConfig {
        let (executable, mut args): (PathBuf, Vec<String>) =
            if let Some((first, rest)) = self.command_prefix.split_first() {
                let mut leading = rest.to_vec();
                leading.push(self.program.clone());
                (PathBuf::from(first), leading)
            } else {
                (PathBuf::from(&self.program), Vec::new())
            };
        args.extend(engine_args);

        let mut config = ProcessConfig::new(executable, working_dir).args(args);
        if let Some(timeout) = timeout {
            config = config.timeout(timeout);
        }
        config
    }
}

/// One engine's metadata and the result of probing it on the current system.
#[derive(Debug, Clone)]
pub struct EngineCapability {
    pub id: EngineId,
    pub name: &'static str,
    pub description: &'static str,
    /// How to run the engine, if a launch could be resolved (override or
    /// PATH probe). `None` means the engine was not found and is not built in.
    pub launch: Option<EngineLaunch>,
    pub version: Option<String>,
    pub built_in: bool,
}

impl EngineCapability {
    pub fn available(&self) -> bool {
        self.built_in
            || self
                .launch
                .as_ref()
                .is_some_and(|launch| !launch.is_empty())
    }
}

/// A specification of how to detect a specific engine and what its primary
/// executable is called on each operating system.
#[derive(Debug, Clone, Copy)]
struct EngineSpec {
    id: EngineId,
    name: &'static str,
    description: &'static str,
    candidate_executables: &'static [&'static str],
    version_arg: Option<&'static str>,
}

/// Version of the bundled `chemx` quantum-chemistry library. chemx exposes no
/// version constant, so keep this in sync with the `chemx` dependency in
/// `Cargo.toml`.
const CHEMX_VERSION: &str = "0.3.0";

const ENGINE_SPECS: &[EngineSpec] = &[EngineSpec {
    id: EngineId::GROMACS,
    name: "GROMACS",
    description: "Molecular dynamics and minimization (gmx).",
    candidate_executables: &["gmx", "gmx_mpi", "gmx_d"],
    version_arg: Some("--version"),
}];

/// Snapshot of detected engines. Re-run [`EngineRegistry::probe`] to refresh
/// availability (for example, after the user edits an override).
#[derive(Debug, Clone, Default)]
pub struct EngineRegistry {
    capabilities: Vec<EngineCapability>,
}

impl EngineRegistry {
    /// Resolve which engines are available, applying the supplied per-engine
    /// launch overrides from configuration.
    ///
    /// This is cheap: it only resolves launches (an override check, or a PATH
    /// lookup for native installs) and never spawns a subprocess. Version
    /// strings are left empty — call [`EngineRegistry::detect_versions`] (or
    /// [`EngineRegistry::probe_with_versions`]) to fill them, which is slow
    /// because it runs each engine's `--version` (a WSL launch can take
    /// seconds to cold-start).
    pub fn probe(overrides: &HashMap<String, EngineLaunch>) -> Self {
        let mut capabilities = Vec::with_capacity(ENGINE_SPECS.len() + 2);
        capabilities.push(EngineCapability {
            id: EngineId::UFF,
            name: "Universal Force Field",
            description: "Molecular-mechanics geometry optimizer using generic UFF parameters.",
            launch: None,
            version: None,
            built_in: true,
        });
        capabilities.push(EngineCapability {
            id: EngineId::CHEMX,
            name: "chemx",
            description: "Pure-Rust quantum chemistry: HF, DFT, MP2, and coupled cluster.",
            launch: None,
            version: Some(CHEMX_VERSION.to_string()),
            built_in: true,
        });

        for spec in ENGINE_SPECS {
            let launch = match overrides.get(spec.id.as_str()) {
                Some(launch) if !launch.is_empty() => Some(launch.clone()),
                _ => probe_native(spec).map(EngineLaunch::native),
            };

            capabilities.push(EngineCapability {
                id: spec.id,
                name: spec.name,
                description: spec.description,
                launch,
                version: None,
                built_in: false,
            });
        }

        Self { capabilities }
    }

    /// [`EngineRegistry::probe`] followed by [`EngineRegistry::detect_versions`].
    /// Slow — spawns each available engine's `--version`. Run only on explicit
    /// user request, not on every settings-panel open.
    pub fn probe_with_versions(overrides: &HashMap<String, EngineLaunch>) -> Self {
        let mut registry = Self::probe(overrides);
        registry.detect_versions();
        registry
    }

    /// Fill in each available engine's version string by running its
    /// `--version`. Slow; a WSL-hosted engine cold-starts the VM.
    pub fn detect_versions(&mut self) {
        for cap in &mut self.capabilities {
            let Some(launch) = cap.launch.clone() else {
                continue;
            };
            let version = ENGINE_SPECS
                .iter()
                .find(|spec| spec.id == cap.id)
                .and_then(|spec| spec.version_arg)
                .and_then(|arg| query_version(&launch, arg));
            if version.is_some() {
                cap.version = version;
            }
        }
    }

    pub fn capabilities(&self) -> &[EngineCapability] {
        &self.capabilities
    }

    pub fn get(&self, id: EngineId) -> Option<&EngineCapability> {
        self.capabilities.iter().find(|cap| cap.id == id)
    }

    pub fn launch(&self, id: EngineId) -> Option<&EngineLaunch> {
        self.get(id).and_then(|cap| cap.launch.as_ref())
    }

    pub fn available(&self, id: EngineId) -> bool {
        self.get(id)
            .map(EngineCapability::available)
            .unwrap_or(false)
    }
}

fn probe_native(spec: &EngineSpec) -> Option<String> {
    spec.candidate_executables.iter().find_map(|candidate| {
        process::find_on_path(candidate).map(|path| path.to_string_lossy().into_owned())
    })
}

/// GROMACS programs to try inside WSL, in priority order. Bare names rely on the
/// WSL login PATH; the absolute path is the conventional install location, used
/// when GROMACS isn't on a non-interactive shell's PATH (GMXRC not sourced).
const WSL_GMX_CANDIDATES: &[&str] = &["gmx", "/usr/local/gromacs/bin/gmx", "gmx_mpi", "gmx_d"];

/// Best-effort auto-detection of a GROMACS launch through WSL — the conventional
/// way GROMACS runs on Windows. Returns `None` when `wsl.exe` is not present
/// (no WSL) or when no candidate `gmx` responds inside it (WSL without GROMACS),
/// letting the caller distinguish those from a working install.
///
/// Slow: spawns `wsl.exe -e <candidate> --version` until one answers (the first
/// call cold-starts the WSL VM). Only call from explicit user actions, never on
/// a settings-panel open. Off Windows this is a cheap no-op (`wsl.exe` is not on
/// PATH).
pub fn detect_wsl_gromacs_launch() -> Option<EngineLaunch> {
    // No WSL on this machine: nothing to detect.
    process::find_on_path("wsl.exe")?;
    WSL_GMX_CANDIDATES.iter().find_map(|candidate| {
        let launch = EngineLaunch {
            command_prefix: vec!["wsl.exe".to_string(), "-e".to_string()],
            program: (*candidate).to_string(),
        };
        wsl_gromacs_responds(&launch).then_some(launch)
    })
}

/// Whether `<launch> --version` actually runs GROMACS: exits cleanly and
/// identifies itself as GROMACS. The identity check rejects false positives
/// from a missing binary, whose error text would otherwise look like output.
fn wsl_gromacs_responds(launch: &EngineLaunch) -> bool {
    let config = launch.to_process_config(
        std::env::temp_dir(),
        ["--version".to_string()],
        Some(Duration::from_secs(20)),
    );
    match process::run(config) {
        Ok(result) => {
            result.success() && format!("{}{}", result.stdout, result.stderr).contains("GROMACS")
        }
        Err(_) => false,
    }
}

/// Run `<launch> <version_arg>` and return the most informative line. Used by
/// the registry on probe and by the settings panel's "Detect" button.
pub fn query_version(launch: &EngineLaunch, arg: &str) -> Option<String> {
    let working_dir = std::env::temp_dir();
    let config = launch.to_process_config(
        working_dir,
        [arg.to_string()],
        Some(Duration::from_secs(15)),
    );
    let result = process::run(config).ok()?;
    let blob = if result.stdout.trim().is_empty() {
        result.stderr
    } else {
        result.stdout
    };
    extract_version(&blob)
}

/// Pull a version string out of a `--version` blob. Prefers a `Label: value`
/// line whose label contains "version" (GROMACS prints
/// `GROMACS version:    2026.2`), which avoids false positives like the
/// echoed `gmx --version` command line. Falls back to the first non-empty
/// line. Also used by `engines::remote` to parse a remote `--version` blob.
pub(crate) fn extract_version(blob: &str) -> Option<String> {
    for line in blob.lines().map(str::trim) {
        if let Some((label, value)) = line.split_once(':')
            && label.to_ascii_lowercase().contains("version")
        {
            let value = value.trim();
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }
    blob.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_always_includes_builtin_uff() {
        let registry = EngineRegistry::probe(&HashMap::new());
        let uff = registry.get(EngineId::UFF).expect("uff entry");
        assert!(uff.available());
        assert!(uff.built_in);
        assert!(uff.launch.is_none());
    }

    #[test]
    fn registry_returns_capability_for_every_spec() {
        let registry = EngineRegistry::probe(&HashMap::new());
        for spec in ENGINE_SPECS {
            assert!(
                registry.get(spec.id).is_some(),
                "missing capability for {}",
                spec.id.as_str()
            );
        }
    }

    #[test]
    fn override_launch_is_honored() {
        let mut overrides = HashMap::new();
        overrides.insert(
            EngineId::GROMACS.as_str().to_string(),
            EngineLaunch {
                command_prefix: vec!["wsl.exe".to_string(), "-e".to_string()],
                program: "/usr/local/gromacs/bin/gmx".to_string(),
            },
        );

        let registry = EngineRegistry::probe(&overrides);
        let gmx = registry.get(EngineId::GROMACS).expect("gromacs entry");
        assert!(gmx.available());
        let launch = gmx.launch.as_ref().expect("launch");
        assert_eq!(launch.program, "/usr/local/gromacs/bin/gmx");
        assert_eq!(launch.command_prefix, vec!["wsl.exe", "-e"]);
    }

    #[test]
    fn native_launch_builds_direct_process_config() {
        let launch = EngineLaunch::native("gmx");
        let config = launch.to_process_config("work", ["--version".to_string()], None);

        assert_eq!(config.executable, PathBuf::from("gmx"));
        assert_eq!(config.args, vec!["--version".to_string()]);
    }

    #[test]
    fn prefixed_launch_threads_program_through_prefix() {
        let launch = EngineLaunch {
            command_prefix: vec!["wsl.exe".to_string(), "-e".to_string()],
            program: "/usr/local/gromacs/bin/gmx".to_string(),
        };
        let config = launch.to_process_config(
            "work",
            ["grompp".to_string(), "-f".to_string(), "em.mdp".to_string()],
            None,
        );

        assert_eq!(config.executable, PathBuf::from("wsl.exe"));
        assert_eq!(
            config.args,
            vec![
                "-e".to_string(),
                "/usr/local/gromacs/bin/gmx".to_string(),
                "grompp".to_string(),
                "-f".to_string(),
                "em.mdp".to_string(),
            ]
        );
    }

    #[test]
    fn display_command_renders_prefix_and_program() {
        let launch = EngineLaunch {
            command_prefix: vec!["wsl.exe".to_string(), "-e".to_string()],
            program: "/usr/local/gromacs/bin/gmx".to_string(),
        };
        assert_eq!(
            launch.display_command(),
            "wsl.exe -e /usr/local/gromacs/bin/gmx"
        );
        assert_eq!(EngineLaunch::native("gmx").display_command(), "gmx");
    }

    #[test]
    fn extract_version_prefers_named_line() {
        let blob = "\
              :-) GROMACS - gmx, 2026.2 (-:\n\
            GROMACS version:    2026.2\n\
            Precision:          mixed\n";
        assert_eq!(extract_version(blob).as_deref(), Some("2026.2"));
    }

    /// Acceptance check for the Windows + GROMACS-in-WSL environment. Ignored
    /// by default so it never fails on machines without WSL/GROMACS; run with
    /// `cargo test --release -- --ignored wsl_gromacs`. This exercises the real
    /// `std::process::Command` path (no git-bash POSIX-path mangling), proving
    /// the `wsl.exe -e <abs-gmx>` launch model end to end.
    #[test]
    #[ignore = "requires GROMACS inside WSL (Windows acceptance environment)"]
    fn wsl_gromacs_is_detected_through_launch() {
        let mut overrides = HashMap::new();
        overrides.insert(
            EngineId::GROMACS.as_str().to_string(),
            EngineLaunch {
                command_prefix: vec!["wsl.exe".to_string(), "-e".to_string()],
                program: "/usr/local/gromacs/bin/gmx".to_string(),
            },
        );

        let registry = EngineRegistry::probe_with_versions(&overrides);
        let gmx = registry.get(EngineId::GROMACS).expect("gromacs capability");
        assert!(
            gmx.available(),
            "GROMACS should be available via WSL launch"
        );
        let version = gmx.version.as_deref().unwrap_or_default();
        assert!(
            version.contains("2026"),
            "expected a 2026.x version, got {version:?}"
        );
    }

    /// Auto-detection of GROMACS-in-WSL with no override configured — the
    /// zero-config Windows path. Ignored by default; run with
    /// `cargo test --release -- --ignored wsl_gromacs_autodetect`.
    #[test]
    #[ignore = "requires GROMACS inside WSL (Windows acceptance environment)"]
    fn wsl_gromacs_autodetect_finds_a_working_launch() {
        let launch = detect_wsl_gromacs_launch().expect("WSL GROMACS should auto-detect");
        assert_eq!(launch.command_prefix, vec!["wsl.exe", "-e"]);
        assert!(
            WSL_GMX_CANDIDATES.contains(&launch.program.as_str()),
            "detected program should be one of the candidates, got {:?}",
            launch.program
        );
    }
}
