#![cfg(feature = "dev-worker")]

//! End-to-end Slurm launcher checks against an SSH-reachable cluster.

use std::time::{Duration, Instant};

use compute_core::domain::{Atom, Structure};
use compute_core::engines::qm::{QmJob, QmKind, QmMethod, QmOptions, QmRequest};
use compute_core::engines::remote::launcher::{
    Launcher, RemoteJobPhase, detect_slurm, retrieve_outcome,
};
use compute_core::engines::remote::{RemoteTarget, deploy, run_probe_command};
use compute_core::hosts::{GpuRequest, JobResources, RemoteHost, SchedulerConfig, SlurmProfile};
use compute_core::wire::{Engine, EngineOutcome, EngineRequest};
use nalgebra::Point3;

fn request() -> EngineRequest {
    let structure = Structure::new(
        "h2",
        vec![
            Atom {
                element: "H".to_string(),
                position: Point3::new(0.0, 0.0, 0.0),
                charge: 0.0,
            },
            Atom {
                element: "H".to_string(),
                position: Point3::new(0.0, 0.0, 0.74),
                charge: 0.0,
            },
        ],
    );
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

fn host() -> Option<RemoteHost> {
    let hostname = std::env::var("SILICOLAB_TEST_SSH_HOST").ok()?;
    let username = std::env::var("SILICOLAB_TEST_SSH_USER").unwrap_or_else(|_| "root".into());
    Some(RemoteHost {
        id: "slurm-test".into(),
        label: "Slurm test".into(),
        hostname,
        username,
        port: 22,
        work_root: "~/.silicolab".into(),
        prelude: Vec::new(),
        engines: Default::default(),
        engine_versions: Default::default(),
        resources: Default::default(),
        scheduler: SchedulerConfig::Slurm(SlurmProfile {
            partition: Some("debug".into()),
            ..Default::default()
        }),
    })
}

fn profile(host: &RemoteHost) -> &SlurmProfile {
    let SchedulerConfig::Slurm(profile) = &host.scheduler else {
        unreachable!()
    };
    profile
}

#[test]
#[ignore = "requires an SSH Slurm host"]
fn slurm_typed_gpu_submit_retrieve_and_cancel_without_sacct() {
    let Some(host) = host() else {
        return;
    };
    let run_uuid = uuid::Uuid::new_v4().to_string();
    let target = RemoteTarget::for_run(&host, &run_uuid);
    let deployed = deploy::ensure_worker_deployed(&host, &target).expect("deploy worker");
    let detection = detect_slurm(&target).expect("detect Slurm");
    assert!(
        !detection.sacct_available,
        "fixture must exercise the scontrol fallback"
    );
    let local_dir = std::env::temp_dir().join(format!("sl-slurm-{run_uuid}"));
    std::fs::create_dir_all(&local_dir).unwrap();
    std::fs::write(
        local_dir.join("request.json"),
        serde_json::to_vec(&request()).unwrap(),
    )
    .unwrap();
    let resources = JobResources {
        cpus_per_task: Some(2),
        memory_mib: Some(256),
        walltime_seconds: Some(120),
        gpu: GpuRequest::Typed {
            gpu_type: "rtx4070".into(),
            count: 1,
        },
        gpu_explicit: true,
    };
    let handle = Launcher::Slurm
        .submit(
            &target,
            &local_dir,
            &deployed.remote_path,
            &resources,
            Some(profile(&host)),
        )
        .expect("submit Slurm job");
    let control = run_probe_command(
        &target,
        &format!("scontrol show job -o {}", handle.id),
        Duration::from_secs(20),
    )
    .expect("read requested TRES");
    // The GPU lands in `TresPerNode`, not `ReqTRES`: `--gres` is a per-node
    // request and Slurm keeps it out of the job's aggregate TRES while pending.
    assert!(control.contains("cpu=2"), "{control}");
    assert!(control.contains("mem=256M"), "{control}");
    assert!(
        control.contains("TresPerNode=gres/gpu:rtx4070:1"),
        "{control}"
    );

    let mut offset = 0;
    let deadline = Instant::now() + Duration::from_secs(90);
    loop {
        let observation = Launcher::Slurm
            .poll(&target, &handle, offset, false)
            .expect("poll Slurm job");
        offset = observation.console.next_offset;
        if observation.phase == RemoteJobPhase::Succeeded {
            break;
        }
        assert!(
            !matches!(
                observation.phase,
                RemoteJobPhase::Failed | RemoteJobPhase::Cancelled | RemoteJobPhase::Lost
            ),
            "unexpected terminal state: {observation:?}"
        );
        assert!(Instant::now() < deadline, "Slurm job timed out");
        std::thread::sleep(Duration::from_millis(250));
    }
    let outcome: EngineOutcome =
        serde_json::from_slice(&retrieve_outcome(&target, &local_dir).expect("retrieve outcome"))
            .unwrap();
    assert!(matches!(outcome, EngineOutcome::Qm(_)));

    let cancel_uuid = uuid::Uuid::new_v4().to_string();
    let cancel_target = RemoteTarget::for_run(&host, &cancel_uuid);
    let cancel_dir = std::env::temp_dir().join(format!("sl-slurm-cancel-{cancel_uuid}"));
    std::fs::create_dir_all(&cancel_dir).unwrap();
    std::fs::write(
        cancel_dir.join("request.json"),
        serde_json::to_vec(&request()).unwrap(),
    )
    .unwrap();
    let cancel_handle = Launcher::Slurm
        .submit(
            &cancel_target,
            &cancel_dir,
            &deployed.remote_path,
            &resources,
            Some(profile(&host)),
        )
        .expect("submit cancellable job");
    Launcher::Slurm
        .cancel(&cancel_target, &cancel_handle)
        .expect("request cancellation");
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let observation = Launcher::Slurm
            .poll(&cancel_target, &cancel_handle, 0, true)
            .expect("poll cancelled job");
        if observation.phase == RemoteJobPhase::Cancelled {
            break;
        }
        assert!(Instant::now() < deadline, "cancellation was not confirmed");
        std::thread::sleep(Duration::from_millis(250));
    }
    let _ = std::fs::remove_dir_all(local_dir);
    let _ = std::fs::remove_dir_all(cancel_dir);
}
