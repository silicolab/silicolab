use nalgebra::Point3;

use super::qm_command;
use crate::{
    domain::{Atom, Structure},
    frontend::state::AppState,
    io::structure_paths::default_structure_save_path,
};

fn water() -> Structure {
    Structure::new(
        "water",
        vec![
            Atom {
                element: "O".to_string(),
                position: Point3::new(0.0, 0.0, 0.1173),
                charge: 0.0,
            },
            Atom {
                element: "H".to_string(),
                position: Point3::new(0.0, 0.7572, -0.4692),
                charge: 0.0,
            },
            Atom {
                element: "H".to_string(),
                position: Point3::new(0.0, -0.7572, -0.4692),
                charge: 0.0,
            },
        ],
    )
}

#[test]
fn qm_recommend_reports_a_level_of_theory() {
    let mut state = AppState::scratch(Default::default(), Vec::new());
    // `recommend` needs no structure and should name a level and a run line.
    let out = qm_command(
        &mut state,
        &["recommend".to_string(), "general".to_string()],
    )
    .expect("qm recommend general should succeed");
    assert!(
        out.contains("level:"),
        "recommendation should name a level: {out}"
    );
    // An unknown task lists the available ones rather than panicking.
    assert!(
        qm_command(
            &mut state,
            &["recommend".to_string(), "nonsense".to_string()]
        )
        .is_err()
    );
}

#[test]
fn qm_optimize_creates_new_entry() {
    let mut state = AppState::scratch(Default::default(), Vec::new());
    let structure = water();
    let save_path = default_structure_save_path(&structure, None);
    let original = state.entries.add_entry(structure, None, save_path);

    let summary = qm_command(
        &mut state,
        &[
            "optimize".to_string(),
            "--method".to_string(),
            "rhf".to_string(),
            "--basis".to_string(),
            "sto-3g".to_string(),
        ],
    )
    .expect("qm optimize should succeed");

    // A heavy QM run produces a *new* entry; the original is preserved.
    assert_ne!(
        Some(original),
        state.entries.active_entry_id(),
        "optimize should create and activate a new entry, not edit in place"
    );
    assert!(
        state.structure().title.contains("opt"),
        "new entry title should mark the optimization: {}",
        state.structure().title
    );
    assert!(summary.contains("geometry optimization"));
}

#[test]
fn build_agent_qm_request_maps_subcommands() {
    let mut state = AppState::scratch(Default::default(), Vec::new());
    let save_path = default_structure_save_path(&water(), None);
    state.entries.add_entry(water(), None, save_path);

    let request = super::build_agent_qm_request(
        &state,
        &[
            "optimize".to_string(),
            "--basis".to_string(),
            "sto-3g".to_string(),
        ],
    )
    .expect("agent qm request should build");
    assert!(matches!(request.kind, super::QmKind::Optimize));
    assert_eq!(request.basis, "sto-3g");
    // Unknown subcommand is rejected.
    assert!(super::build_agent_qm_request(&state, &["bogus".to_string()]).is_err());
}

#[test]
fn qm_ts_subcommand_builds_a_coordinate_scan() {
    use crate::engines::qm::{QmInternalCoordinate, QmTsGuess};
    let mut state = AppState::scratch(Default::default(), Vec::new());
    let save_path = default_structure_save_path(&water(), None);
    state.entries.add_entry(water(), None, save_path);

    let request = super::build_agent_qm_request(
        &state,
        &[
            "ts".to_string(),
            "--scan-bond".to_string(),
            "1,3".to_string(),
            "--scan-from".to_string(),
            "0.9".to_string(),
            "--scan-to".to_string(),
            "1.6".to_string(),
            "--basis".to_string(),
            "sto-3g".to_string(),
        ],
    )
    .expect("qm ts coordinate scan should build");
    assert!(matches!(request.kind, super::QmKind::TransitionState));
    match request.ts.expect("ts config").guess {
        QmTsGuess::CoordinateScan(scan) => {
            assert_eq!(scan.coordinate, QmInternalCoordinate::Bond(1, 3));
            assert_eq!(scan.start, 0.9);
        }
        other => panic!("expected a coordinate scan, got {other:?}"),
    }

    // A bare `qm ts` is a single-guess search from the current geometry.
    let single = super::build_agent_qm_request(
        &state,
        &[
            "ts".to_string(),
            "--basis".to_string(),
            "sto-3g".to_string(),
        ],
    )
    .expect("bare qm ts should build");
    assert!(matches!(
        single.ts.expect("ts config").guess,
        QmTsGuess::Single
    ));
}

#[test]
fn agent_qm_request_runs_off_thread() {
    use crate::frontend::jobs::{QmWorkerMessage, spawn_qm_job};
    use std::time::{Duration, Instant};

    let mut state = AppState::scratch(Default::default(), Vec::new());
    let save_path = default_structure_save_path(&water(), None);
    state.entries.add_entry(water(), None, save_path);

    let request = super::build_agent_qm_request(
        &state,
        &[
            "energy".to_string(),
            "--method".to_string(),
            "rhf".to_string(),
            "--basis".to_string(),
            "sto-3g".to_string(),
        ],
    )
    .expect("request builds");

    // Spawn the same job the agent's heavy path uses and poll it to
    // completion, exactly as `poll_heavy_qm` does (minus the agent loop).
    let job = spawn_qm_job(crate::engines::qm::QmJob::Molecular(request), None);
    let deadline = Instant::now() + Duration::from_secs(120);
    let mut summary = None;
    while Instant::now() < deadline {
        match job.receiver.try_recv() {
            Ok(QmWorkerMessage::Finished(outcome)) => {
                summary = Some(outcome.summary);
                break;
            }
            Ok(QmWorkerMessage::Failed(error)) => panic!("qm job failed: {error}"),
            Ok(QmWorkerMessage::Progress { .. }) => {}
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => break,
        }
    }
    assert!(
        summary.is_some(),
        "async qm job should finish with a summary"
    );
}

#[test]
fn qm_energy_small_molecule_passes_the_memory_guard() {
    let mut state = AppState::scratch(Default::default(), Vec::new());
    let save_path = default_structure_save_path(&water(), None);
    state.entries.add_entry(water(), None, save_path);
    // sto-3g water is ~7 basis functions → kilobytes; the guard must not trip.
    qm_command(
        &mut state,
        &[
            "energy".to_string(),
            "--basis".to_string(),
            "sto-3g".to_string(),
        ],
    )
    .expect("small in-core energy should pass the memory guard and run");
}

#[test]
fn qm_energy_does_not_create_entry() {
    let mut state = AppState::scratch(Default::default(), Vec::new());
    let structure = water();
    let save_path = default_structure_save_path(&structure, None);
    let original = state.entries.add_entry(structure, None, save_path);

    qm_command(
        &mut state,
        &[
            "energy".to_string(),
            "--basis".to_string(),
            "sto-3g".to_string(),
        ],
    )
    .expect("qm energy should succeed");

    // A single point changes nothing in the entry list.
    assert_eq!(Some(original), state.entries.active_entry_id());
}
