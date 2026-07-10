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

use std::{path::PathBuf, time::Duration};

use crate::engines::process::{self, ProcessConfig};

pub use crate::launch::{Compute, ComputeResources, EngineId, EngineLaunch, EngineLaunches};

impl EngineLaunch {
    /// Build a [`ProcessConfig`] that runs this engine with `engine_args` in
    /// `working_dir`. The prefix's first token becomes the spawned executable;
    /// remaining prefix tokens, the program, and the engine args follow.
    ///
    /// This conversion lives here, not beside the struct: [`EngineLaunch`] sits at
    /// a leaf layer so `hosts` can store one without importing `engines`, while
    /// `ProcessConfig` belongs to `engines`.
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

/// What is known about one engine on one target. The distinction the UI lives or
/// dies by is [`EngineStatus::Unverified`] vs [`EngineStatus::Verified`]: a launch
/// exists either way, but only the latter has been run and answered for itself.
///
/// A failed verification is deliberately absent. It is a fact about the target
/// *right now*, it decays faster than a success, and replaying a week-old failure
/// on app start is worse than admitting the launch is simply unverified. Failures
/// live in the caller's session state, keyed by the launch that produced them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EngineStatus {
    /// Compiled into SilicoLab; nothing to configure and nothing to verify.
    BuiltIn { version: Option<String> },
    /// No launch configured, and none found on the target.
    NotConfigured,
    /// A launch exists (configured, or found on the target's PATH) but has never
    /// been run. Whether it works is unknown.
    Unverified { launch: EngineLaunch },
    /// This exact launch ran and identified itself as this engine.
    Verified {
        launch: EngineLaunch,
        version: String,
        checked_at: u64,
    },
}

impl EngineStatus {
    pub fn launch(&self) -> Option<&EngineLaunch> {
        match self {
            Self::Unverified { launch } | Self::Verified { launch, .. } => Some(launch),
            Self::BuiltIn { .. } | Self::NotConfigured => None,
        }
    }

    pub fn version(&self) -> Option<&str> {
        match self {
            Self::BuiltIn { version } => version.as_deref(),
            Self::Verified { version, .. } => Some(version),
            Self::NotConfigured | Self::Unverified { .. } => None,
        }
    }

    pub fn built_in(&self) -> bool {
        matches!(self, Self::BuiltIn { .. })
    }
}

/// One engine's metadata and what is known about it on the current system.
#[derive(Debug, Clone)]
pub struct EngineCapability {
    pub id: EngineId,
    pub name: &'static str,
    pub description: &'static str,
    pub status: EngineStatus,
}

/// How to detect one external engine: what its executable may be called, how to
/// ask it for a version, and how to tell its answer apart from an unrelated
/// binary's. The single source of truth for every probe — local PATH, WSL, and
/// SSH to a remote host all read this.
#[derive(Debug, Clone, Copy)]
pub struct EngineSpec {
    pub id: EngineId,
    pub name: &'static str,
    pub description: &'static str,
    /// Programs to try, in priority order: bare names (resolved through PATH, or
    /// through a remote host's prelude) then conventional absolute install paths.
    /// An absolute POSIX path is inert on a native Windows probe — it simply is
    /// not a file — so one list serves every platform.
    pub candidate_executables: &'static [&'static str],
    pub version_arg: Option<&'static str>,
    /// A substring the engine's own `version_arg` output always contains. Guards
    /// against a same-named binary that is not this engine, and against a missing
    /// binary whose error text would otherwise look like output.
    pub identity_marker: &'static str,
}

const ENGINE_SPECS: &[EngineSpec] = &[EngineSpec {
    id: EngineId::GROMACS,
    name: "GROMACS",
    description: "Molecular dynamics and minimization (gmx).",
    candidate_executables: &["gmx", "/usr/local/gromacs/bin/gmx", "gmx_mpi", "gmx_d"],
    version_arg: Some("--version"),
    identity_marker: "GROMACS",
}];

/// The detection spec for `id`, or `None` for a built-in engine (no executable).
pub fn engine_spec(id: EngineId) -> Option<&'static EngineSpec> {
    ENGINE_SPECS.iter().find(|spec| spec.id == id)
}

/// Every engine that has an executable to configure, in panel order. Built-ins are
/// not here: there is nothing to point at and nothing to verify.
pub fn external_engine_ids() -> impl Iterator<Item = EngineId> {
    ENGINE_SPECS.iter().map(|spec| spec.id)
}

/// The external engine specs, for a panel that renders one editor per engine.
pub fn external_engine_specs() -> &'static [EngineSpec] {
    ENGINE_SPECS
}

/// Snapshot of detected engines. Re-run [`EngineRegistry::probe`] to refresh
/// availability (for example, after the user edits an override).
#[derive(Debug, Clone, Default)]
pub struct EngineRegistry {
    capabilities: Vec<EngineCapability>,
}

impl EngineRegistry {
    /// Resolve what is known about each engine, reading the supplied per-engine
    /// launches from configuration.
    ///
    /// Cheap: it resolves launches (a config lookup, or a PATH lookup for native
    /// installs) and never spawns a subprocess. It therefore never reports
    /// [`EngineStatus::Verified`] on its own — a verification can only come from
    /// running the engine, which [`crate::engines::registry::verify_launch`] does
    /// on an explicit user action, off the UI thread.
    pub fn probe(overrides: &EngineLaunches) -> Self {
        let mut capabilities = Vec::with_capacity(ENGINE_SPECS.len() + 3);
        capabilities.push(EngineCapability {
            id: EngineId::UFF,
            name: "Universal Force Field",
            description: "Molecular-mechanics geometry optimizer using generic UFF parameters.",
            status: EngineStatus::BuiltIn { version: None },
        });
        capabilities.push(EngineCapability {
            id: EngineId::HARTREE,
            name: "hartree",
            description: "Pure-Rust quantum chemistry: molecular HF, DFT, MP2, and coupled cluster, plus periodic (crystalline) DFT.",
            status: EngineStatus::BuiltIn { version: Some(hartree::VERSION.to_string()) },
        });
        capabilities.push(EngineCapability {
            id: EngineId::DOCKING,
            name: "Vina docking",
            description: "Pure-Rust molecular docking: an AutoDock Vina reimplementation for ligand-receptor pose search and scoring.",
            status: EngineStatus::BuiltIn {
                version: Some(format!(
                    "AutoDock Vina {} compatible",
                    docking::REFERENCE_VINA_VERSION
                )),
            },
        });

        for spec in ENGINE_SPECS {
            capabilities.push(EngineCapability {
                id: spec.id,
                name: spec.name,
                description: spec.description,
                status: local_status(overrides, spec),
            });
        }

        Self { capabilities }
    }

    pub fn capabilities(&self) -> &[EngineCapability] {
        &self.capabilities
    }

    pub fn get(&self, id: EngineId) -> Option<&EngineCapability> {
        self.capabilities.iter().find(|cap| cap.id == id)
    }

    pub fn launch(&self, id: EngineId) -> Option<&EngineLaunch> {
        self.get(id).and_then(|cap| cap.status.launch())
    }

    pub fn status(&self, id: EngineId) -> Option<&EngineStatus> {
        self.get(id).map(|cap| &cap.status)
    }
}

/// What configuration alone can say about `spec` on this machine: a configured
/// launch (with its verification, if that verification was taken against the launch
/// still configured), else a PATH hit, else nothing.
fn local_status(overrides: &EngineLaunches, spec: &EngineSpec) -> EngineStatus {
    if let Some(entry) = overrides.entry(spec.id) {
        return match &entry.verified {
            Some(verified) => EngineStatus::Verified {
                launch: entry.launch.clone(),
                version: verified.version.clone(),
                checked_at: verified.checked_at,
            },
            None => EngineStatus::Unverified {
                launch: entry.launch.clone(),
            },
        };
    }
    // A binary sitting on PATH is a launch, not a proof: it has not been run.
    match probe_native(spec) {
        Some(program) => EngineStatus::Unverified {
            launch: EngineLaunch::native(program),
        },
        None => EngineStatus::NotConfigured,
    }
}

/// The first of `spec`'s candidates that exists on PATH (or as an absolute path).
/// A cheap filesystem lookup — it does not run the engine.
pub fn probe_native(spec: &EngineSpec) -> Option<String> {
    spec.candidate_executables.iter().find_map(|candidate| {
        process::find_on_path(candidate).map(|path| path.to_string_lossy().into_owned())
    })
}

/// Choose a plausible launch without running the engine. Native candidates are
/// preferred; on Windows, an available WSL launcher falls back to the first
/// conventional engine command inside the distribution.
pub fn local_launch_candidate(spec: &EngineSpec) -> Option<EngineLaunch> {
    if let Some(program) = probe_native(spec) {
        return Some(EngineLaunch::native(program));
    }
    process::find_on_path("wsl.exe")?;
    spec.candidate_executables
        .first()
        .map(|program| EngineLaunch {
            command_prefix: vec!["wsl.exe".to_string(), "-e".to_string()],
            program: (*program).to_string(),
        })
}

/// Run `<launch> <version_arg>` on this machine and decide whether it *is* this
/// engine: it must exit cleanly and print the spec's identity marker. On success
/// returns the version it reported; on failure, a reason phrased for the settings
/// panel, distinguishing "would not start" from "started, but is not this engine".
///
/// Slow — a WSL-prefixed launch cold-starts the VM. Never call from the UI thread.
/// The remote twin of this is `engines::remote::verify_remote_launch`.
pub fn verify_launch(launch: &EngineLaunch, spec: &EngineSpec) -> Result<String, String> {
    let Some(version_arg) = spec.version_arg else {
        return Err(format!("{} has no version check", spec.name));
    };
    let config = launch.to_process_config(
        std::env::temp_dir(),
        [version_arg.to_string()],
        Some(Duration::from_secs(20)),
    );
    let result = process::run(config).map_err(|error| format!("could not run it: {error}"))?;
    let blob = format!("{}{}", result.stdout, result.stderr);
    if !result.success() {
        return Err(format!(
            "`{} {version_arg}` failed: {}",
            launch.display_command(),
            first_line(&blob).unwrap_or("no output")
        ));
    }
    if !blob.contains(spec.identity_marker) {
        return Err(format!(
            "it runs, but does not identify itself as {}",
            spec.name
        ));
    }
    Ok(extract_version(&blob).unwrap_or_else(|| spec.name.to_string()))
}

/// Find a working launch for `spec` on this machine when none is configured: each
/// candidate on PATH, then each candidate behind `wsl.exe -e` (the conventional way
/// GROMACS runs on Windows; off Windows `wsl.exe` is simply not on PATH). Every
/// candidate is *verified*, so a hit is a launch plus the version it reported.
///
/// Slow, for the same reason [`verify_launch`] is. Never call from the UI thread.
pub fn probe_local(spec: &EngineSpec) -> Option<(EngineLaunch, String)> {
    let native = probe_native(spec)
        .map(EngineLaunch::native)
        .and_then(|launch| verify_launch(&launch, spec).ok().map(|v| (launch, v)));
    if native.is_some() {
        return native;
    }
    process::find_on_path("wsl.exe")?;
    spec.candidate_executables.iter().find_map(|candidate| {
        let launch = EngineLaunch {
            command_prefix: vec!["wsl.exe".to_string(), "-e".to_string()],
            program: (*candidate).to_string(),
        };
        verify_launch(&launch, spec).ok().map(|v| (launch, v))
    })
}

fn first_line(blob: &str) -> Option<&str> {
    blob.lines().map(str::trim).find(|line| !line.is_empty())
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
        let registry = EngineRegistry::probe(&EngineLaunches::new());
        let uff = registry.get(EngineId::UFF).expect("uff entry");
        assert!(uff.status.built_in());
        assert!(uff.status.launch().is_none());
    }

    #[test]
    fn registry_returns_capability_for_every_spec() {
        let registry = EngineRegistry::probe(&EngineLaunches::new());
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
        let mut overrides = EngineLaunches::new();
        overrides.insert(
            EngineId::GROMACS,
            EngineLaunch {
                command_prefix: vec!["wsl.exe".to_string(), "-e".to_string()],
                program: "/usr/local/gromacs/bin/gmx".to_string(),
            },
        );

        let registry = EngineRegistry::probe(&overrides);
        let gmx = registry.get(EngineId::GROMACS).expect("gromacs entry");
        let launch = gmx.status.launch().expect("launch");
        assert_eq!(launch.program, "/usr/local/gromacs/bin/gmx");
        assert_eq!(launch.command_prefix, vec!["wsl.exe", "-e"]);
    }

    /// Configuring a path that cannot possibly run must never read as working.
    /// `probe` does not spawn anything, so the strongest thing it may ever say
    /// about a configured launch is "unverified" — and `verify_launch`, which does
    /// spawn, must reject it outright.
    #[test]
    fn a_nonsense_program_is_unverified_and_fails_verification() {
        let mut overrides = EngineLaunches::new();
        overrides.insert(EngineId::GROMACS, EngineLaunch::native("/nonsense/gmx"));

        let registry = EngineRegistry::probe(&overrides);
        let status = registry.status(EngineId::GROMACS).expect("gromacs status");
        assert!(
            matches!(status, EngineStatus::Unverified { .. }),
            "a configured-but-unrun launch is unverified, got {status:?}"
        );
        assert!(status.version().is_none());

        let spec = engine_spec(EngineId::GROMACS).expect("gromacs spec");
        let reason = verify_launch(&EngineLaunch::native("/nonsense/gmx"), spec)
            .expect_err("a nonexistent program cannot verify");
        assert!(!reason.is_empty());
    }

    /// A verification taken against the configured launch is what makes it Verified —
    /// the registry reads the proof, it never invents one.
    #[test]
    fn a_verified_entry_surfaces_its_version() {
        let mut overrides = EngineLaunches::new();
        overrides.insert_verified(EngineId::GROMACS, EngineLaunch::native("gmx"), "2026.2");

        let registry = EngineRegistry::probe(&overrides);
        assert_eq!(
            registry
                .status(EngineId::GROMACS)
                .and_then(EngineStatus::version),
            Some("2026.2")
        );
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
    fn wsl_gromacs_verifies_through_its_launch() {
        let spec = engine_spec(EngineId::GROMACS).expect("gromacs spec");
        let launch = EngineLaunch {
            command_prefix: vec!["wsl.exe".to_string(), "-e".to_string()],
            program: "/usr/local/gromacs/bin/gmx".to_string(),
        };
        let version = verify_launch(&launch, spec).expect("GROMACS should verify via WSL launch");
        assert!(
            version.contains("2026"),
            "expected a 2026.x version, got {version:?}"
        );
    }

    /// The user types a `gmx` that is *not* one of the spec's candidates — exactly
    /// the case auto-detection cannot reach, and therefore the case that used to be
    /// unverifiable. Verification must check what is configured, not go looking.
    /// Set `SILICOLAB_TEST_WSL_GMX` to such an absolute path inside WSL.
    #[test]
    #[ignore = "requires a non-standard GROMACS inside WSL (set SILICOLAB_TEST_WSL_GMX)"]
    fn a_wsl_gmx_outside_the_candidate_list_still_verifies() {
        let Ok(program) = std::env::var("SILICOLAB_TEST_WSL_GMX") else {
            eprintln!("skip: set SILICOLAB_TEST_WSL_GMX to an absolute gmx path inside WSL");
            return;
        };
        let spec = engine_spec(EngineId::GROMACS).expect("gromacs spec");
        assert!(
            !spec.candidate_executables.contains(&program.as_str()),
            "this test is only meaningful for a path auto-detection would never try"
        );
        let launch = EngineLaunch {
            command_prefix: vec!["wsl.exe".to_string(), "-e".to_string()],
            program,
        };
        verify_launch(&launch, spec).expect("a user-configured gmx must verify");
    }

    /// Auto-detection of GROMACS-in-WSL with no launch configured — the zero-config
    /// Windows path. A hit carries the version, because probing verifies.
    #[test]
    #[ignore = "requires GROMACS inside WSL (Windows acceptance environment)"]
    fn wsl_gromacs_autodetect_finds_a_verified_launch() {
        let spec = engine_spec(EngineId::GROMACS).expect("gromacs spec");
        let (launch, version) = probe_local(spec).expect("WSL GROMACS should auto-detect");
        assert!(!version.is_empty());
        assert!(
            spec.candidate_executables
                .contains(&launch.program.as_str()),
            "detected program should be one of the candidates, got {:?}",
            launch.program
        );
    }
}
