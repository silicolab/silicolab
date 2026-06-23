//! The hidden engine-exec subcommand, end to end on the real binary.
//!
//! `silicolab exec <request.json> <outcome.json>` is the engine entry the
//! local-subprocess executor self-execs (and a remote worker later runs). This
//! drives it through the actual built binary — exercising the `main.rs` dispatch
//! that must run before the script path — and checks the result agrees with an
//! in-process run to SCF convergence tolerance, not bit-for-bit.

use std::process::Command;

use silicolab::engines::qm::{QmJob, QmKind, QmMethod, QmOptions, QmRequest};
use silicolab::io::formats::xyz::parse_xyz;
use silicolab::wire::{Engine, EngineOutcome, EngineRequest, Executor, JobUpdate, run_job};

fn h2_request() -> EngineRequest {
    let structure = parse_xyz("2\nh2\nH 0.0 0.0 0.0\nH 0.0 0.0 0.74\n").expect("parse h2");
    EngineRequest::new(Engine::Qm(QmJob::Molecular(QmRequest {
        structure,
        method: QmMethod::Rhf,
        basis: "sto-3g".to_string(),
        charge: 0,
        multiplicity: 1,
        kind: QmKind::SinglePoint,
        options: QmOptions::default(),
        ts: None,
    })))
}

fn in_process_energy(request: EngineRequest) -> f64 {
    let running = run_job(request, Executor::InProcess);
    loop {
        match running.updates().recv().expect("worker stays alive") {
            JobUpdate::Finished(outcome) => {
                let EngineOutcome::Qm(outcome) = *outcome else {
                    panic!("expected a QM outcome");
                };
                return outcome.energy_hartree;
            }
            JobUpdate::Failed(error) => panic!("in-process job failed: {error}"),
            JobUpdate::Progress { .. } => {}
        }
    }
}

#[test]
fn exec_subcommand_matches_in_process_within_tolerance() {
    let request = h2_request();
    let expected = in_process_energy(request.clone());

    let dir = std::env::temp_dir().join("silicolab-it-engine-exec");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create scratch dir");
    let request_path = dir.join("request.json");
    let outcome_path = dir.join("outcome.json");
    std::fs::write(&request_path, serde_json::to_vec(&request).unwrap()).expect("write request");

    let status = Command::new(env!("CARGO_BIN_EXE_silicolab"))
        .arg("exec")
        .arg(&request_path)
        .arg(&outcome_path)
        .status()
        .expect("run the exec subcommand");
    assert!(status.success(), "exec subcommand exited with {status}");

    let bytes = std::fs::read(&outcome_path).expect("read outcome");
    let EngineOutcome::Qm(outcome) = serde_json::from_slice(&bytes).expect("parse outcome") else {
        panic!("expected a QM outcome");
    };
    assert!(outcome.converged, "exec outcome did not converge");
    assert!(
        (expected - outcome.energy_hartree).abs() < 1e-6,
        "in-process {expected} vs exec {} exceeded SCF tolerance",
        outcome.energy_hartree
    );

    let _ = std::fs::remove_dir_all(&dir);
}
