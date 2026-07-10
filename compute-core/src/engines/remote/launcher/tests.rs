use super::*;

fn target() -> RemoteTarget {
    RemoteTarget {
        host_id: "h".into(),
        host_label: "H".into(),
        user: "u".into(),
        host: "host".into(),
        port: 22,
        key_path: "key".into(),
        known_hosts_path: "known".into(),
        prelude: Vec::new(),
        scheduler_prelude: vec!["module load slurm".into()],
        remote_dir: "~/.silicolab/runs/abcdef12-1".into(),
        connect_timeout_secs: 15,
    }
}

#[test]
fn launcher_tokens_round_trip() {
    for launcher in [Launcher::Direct, Launcher::Slurm] {
        assert_eq!(Launcher::from_token(launcher.token()).unwrap(), launcher);
    }
    assert!(Launcher::from_token("other").is_err());
}

#[test]
fn run_script_writes_an_atomic_exit_marker() {
    let script = render_run_sh("/opt/worker", &["module load x".into()]);
    assert!(script.contains("/opt/worker exec request.json outcome.json"));
    assert!(script.contains("run.exit.tmp.$$"));
    assert!(script.contains("mv run.exit.tmp.$$ run.exit"));
}

#[test]
fn parses_sbatch_handles() {
    assert_eq!(
        parse_sbatch_output("123\n").unwrap(),
        LaunchHandle {
            id: "123".into(),
            cluster: None
        }
    );
    assert_eq!(
        parse_sbatch_output("123;alpha\n").unwrap(),
        LaunchHandle {
            id: "123".into(),
            cluster: Some("alpha".into())
        }
    );
    for value in ["", "abc", "12;bad name", "12;a;b", "error 12"] {
        assert!(parse_sbatch_output(value).is_err(), "{value}");
    }
}

#[test]
fn sbatch_renders_resources_and_quotes_values() {
    let resources = JobResources {
        cpus_per_task: Some(4),
        memory_mib: Some(2048),
        walltime_seconds: Some(90_061),
        gpu: GpuRequest::Typed {
            gpu_type: "a100".into(),
            count: 2,
        },
        gpu_explicit: true,
    };
    let profile = SlurmProfile {
        partition: Some("debug".into()),
        account: Some("lab".into()),
        ..Default::default()
    };
    let command = render_sbatch_command(&target(), &resources, &profile).unwrap();
    for expected in [
        "'--cpus-per-task=4'",
        "'--mem=2048M'",
        "'--time=1-01:01:01'",
        "'--partition=debug'",
        "'--account=lab'",
        "'--gres=gpu:a100:2'",
    ] {
        assert!(command.contains(expected), "{command}");
    }
    assert!(command.contains("--chdir=\"$PWD\""));
}

#[test]
fn gpu_syntax_supports_none_any_typed_and_gpus() {
    let gres = SlurmGpuSyntax::default();
    assert_eq!(render_gpu_argument(&GpuRequest::None, &gres).unwrap(), None);
    assert_eq!(
        render_gpu_argument(&GpuRequest::Any { count: 2 }, &gres)
            .unwrap()
            .as_deref(),
        Some("--gres=gpu:2")
    );
    assert_eq!(
        render_gpu_argument(
            &GpuRequest::Typed {
                gpu_type: "rtx4070".into(),
                count: 1
            },
            &gres
        )
        .unwrap()
        .as_deref(),
        Some("--gres=gpu:rtx4070:1")
    );
    assert_eq!(
        render_gpu_argument(
            &GpuRequest::Typed {
                gpu_type: "a100".into(),
                count: 2
            },
            &SlurmGpuSyntax::Gpus
        )
        .unwrap()
        .as_deref(),
        Some("--gpus=a100:2")
    );
}

#[test]
fn slurm_state_parsers_are_safe_and_complete() {
    assert_eq!(
        parse_squeue_line("PENDING|Resources").unwrap(),
        ("PENDING".into(), Some("Resources".into()), None)
    );
    assert_eq!(
        parse_sacct_line("42|FAILED+|1:0|OutOfMemory", "42").unwrap(),
        ("FAILED".into(), Some("OutOfMemory".into()), Some(1))
    );
    assert_eq!(
        parse_scontrol_line("JobId=42 JobState=COMPLETED Reason=None ExitCode=0:0").unwrap(),
        ("COMPLETED".into(), None, Some(0))
    );
    assert!(parse_sacct_line("bad", "42").is_err());
}

#[test]
fn maps_slurm_phases() {
    assert_eq!(
        map_slurm_state("PENDING", false, None),
        RemoteJobPhase::Queued
    );
    assert_eq!(
        map_slurm_state("CONFIGURING", false, None),
        RemoteJobPhase::Starting
    );
    assert_eq!(
        map_slurm_state("RUNNING", false, None),
        RemoteJobPhase::Running
    );
    assert_eq!(
        map_slurm_state("COMPLETING", false, None),
        RemoteJobPhase::Completing
    );
    assert_eq!(
        map_slurm_state("RUNNING", true, None),
        RemoteJobPhase::Cancelling
    );
    assert_eq!(
        map_slurm_state("CANCELLED", true, Some(0)),
        RemoteJobPhase::Cancelled
    );
    assert_eq!(
        map_slurm_state("COMPLETED", false, Some(0)),
        RemoteJobPhase::Succeeded
    );
    assert_eq!(
        map_slurm_state("COMPLETED", false, None),
        RemoteJobPhase::Succeeded
    );
    assert_eq!(
        map_slurm_state("COMPLETED", false, Some(1)),
        RemoteJobPhase::Failed
    );
    for state in [
        "BOOT_FAIL",
        "DEADLINE",
        "FAILED",
        "NODE_FAIL",
        "OUT_OF_MEMORY",
        "PREEMPTED",
        "SPECIAL_EXIT",
        "TIMEOUT",
    ] {
        assert_eq!(
            map_slurm_state(state, false, Some(1)),
            RemoteJobPhase::Failed
        );
    }
    assert_eq!(
        map_slurm_state("FUTURE", false, None),
        RemoteJobPhase::Unknown
    );
}

#[test]
fn parses_incremental_console_chunks() {
    let observation =
        parse_direct_observation("SL_DIRECT RUNNING\nSL_CONSOLE 4 9\nhello", false).unwrap();
    assert_eq!(observation.phase, RemoteJobPhase::Running);
    assert_eq!(observation.console.text, "hello");
    assert_eq!(observation.console.next_offset, 9);
}

#[test]
fn a_killed_direct_job_reads_as_cancelled_only_while_cancelling() {
    let lost = parse_direct_observation("SL_DIRECT LOST\nSL_CONSOLE 0 0\n", false).unwrap();
    assert_eq!(lost.phase, RemoteJobPhase::Lost);
    let cancelled = parse_direct_observation("SL_DIRECT LOST\nSL_CONSOLE 0 0\n", true).unwrap();
    assert_eq!(cancelled.phase, RemoteJobPhase::Cancelled);
}

#[test]
fn completed_without_an_outcome_is_failed() {
    let observation = parse_slurm_observation(
        "SLURM_MARKER FAILED 0 missing_outcome\nSL_CONSOLE 0 0\n",
        "42",
        false,
    )
    .unwrap();
    assert_eq!(observation.phase, RemoteJobPhase::Failed);
    assert_eq!(observation.reason.as_deref(), Some("missing_outcome"));
}

#[test]
fn parses_sinfo_capabilities() {
    let capabilities = parse_sinfo("debug*|gpu:rtx4070:1|avx2|idle\ncpu|(null)|zen4,avx2|mix\n");
    assert_eq!(capabilities.partitions, ["cpu", "debug"]);
    assert_eq!(capabilities.gpu_types, ["rtx4070"]);
    assert_eq!(capabilities.features, ["avx2", "zen4"]);
}
