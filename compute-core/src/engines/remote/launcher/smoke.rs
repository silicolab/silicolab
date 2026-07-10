use std::time::Duration;

use anyhow::{Result, bail};

use super::{
    CONSOLE_FILE, EXIT_FILE, GpuRequest, JobResources, OUTCOME_FILE, RemoteJobPhase, RemoteTarget,
    SLURM_SCRIPT, SlurmProfile, run_scheduler_command, slurm_cancel, slurm_poll, slurm_submit,
};

pub fn slurm_smoke_test(
    target: &RemoteTarget,
    profile: &SlurmProfile,
    worker_path: &str,
) -> Result<String> {
    let body = format!(
        "#!/bin/sh\nset +e\nif [ -x {worker_path} ]; then {worker_path} --version > outcome.json 2>&1; else echo 'deployed worker is not visible on the compute node' >&2; false; fi\ncode=$?\nprintf '%s\\n' \"$code\" > run.exit.tmp.$$ && mv run.exit.tmp.$$ run.exit\nexit \"$code\"\n"
    );
    let setup = format!(
        "mkdir -p {dir} && cd {dir} && rm -f {EXIT_FILE} {OUTCOME_FILE} {CONSOLE_FILE} && printf %s {body} > {script} && chmod 700 {script}",
        dir = target.remote_dir,
        body = super::super::sh_quote(&body),
        script = SLURM_SCRIPT,
    );
    run_scheduler_command(
        target,
        &setup,
        Duration::from_secs(30),
        "prepare the Slurm test",
    )?;
    let resources = JobResources {
        cpus_per_task: Some(1),
        memory_mib: Some(64),
        walltime_seconds: Some(120),
        gpu: GpuRequest::None,
        gpu_explicit: true,
    };
    let handle = slurm_submit(target, &resources, profile)?;
    let mut offset = 0;
    for _ in 0..60 {
        let observation = slurm_poll(target, &handle, offset, false)?;
        offset = observation.console.next_offset;
        match observation.phase {
            RemoteJobPhase::Succeeded => {
                let command = format!("cd {} && cat {OUTCOME_FILE}", target.remote_dir);
                return run_scheduler_command(
                    target,
                    &command,
                    Duration::from_secs(20),
                    "read the Slurm test result",
                );
            }
            RemoteJobPhase::Failed | RemoteJobPhase::Lost | RemoteJobPhase::Cancelled => {
                bail!(
                    "Slurm test ended as {:?}: {}",
                    observation.phase,
                    observation.reason.unwrap_or(observation.console.text)
                )
            }
            _ => std::thread::sleep(Duration::from_secs(1)),
        }
    }
    let _ = slurm_cancel(target, &handle);
    bail!("Slurm test did not finish within 60 seconds")
}
