//! Shared remote bundle launchers for Direct SSH and Slurm.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};

use super::RemoteTarget;
use crate::engines::process;
use crate::hosts::{GpuRequest, JobResources, SlurmGpuSyntax, SlurmProfile};

mod smoke;
pub use smoke::slurm_smoke_test;

const RUN_SCRIPT: &str = "run.sh";
const SLURM_SCRIPT: &str = "slurm-job.sh";
const PGID_FILE: &str = "run.pgid";
const EXIT_FILE: &str = "run.exit";
const CONSOLE_FILE: &str = "run.console";
pub const REQUEST_FILE: &str = "request.json";
pub const OUTCOME_FILE: &str = "outcome.json";

#[derive(Debug, Clone)]
pub struct RemoteExecution {
    pub target: RemoteTarget,
    pub launcher: Launcher,
    pub working_dir: PathBuf,
    pub worker_path: String,
    pub resources: JobResources,
    pub slurm_profile: Option<SlurmProfile>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Launcher {
    Direct,
    Slurm,
}

impl Launcher {
    pub fn token(self) -> &'static str {
        match self {
            Self::Direct => "direct",
            Self::Slurm => "slurm",
        }
    }

    pub fn from_token(token: &str) -> Result<Self> {
        match token {
            "direct" => Ok(Self::Direct),
            "slurm" => Ok(Self::Slurm),
            _ => bail!("unknown remote scheduler `{token}`"),
        }
    }

    pub fn submit(
        self,
        target: &RemoteTarget,
        working_dir: &Path,
        worker_path: &str,
        resources: &JobResources,
        slurm_profile: Option<&SlurmProfile>,
    ) -> Result<LaunchHandle> {
        resources.validate()?;
        write_run_sh(working_dir, worker_path, &target.prelude)?;
        if self == Self::Slurm {
            std::fs::write(working_dir.join(SLURM_SCRIPT), render_slurm_script())?;
        }
        super::sync_up(target, working_dir)?;
        match self {
            Self::Direct => direct_submit(target),
            Self::Slurm => slurm_submit(
                target,
                resources,
                slurm_profile.ok_or_else(|| anyhow::anyhow!("Slurm profile is missing"))?,
            ),
        }
    }

    pub fn poll(
        self,
        target: &RemoteTarget,
        handle: &LaunchHandle,
        console_offset: u64,
        cancelling: bool,
    ) -> Result<LauncherObservation> {
        match self {
            Self::Direct => direct_poll(target, console_offset, cancelling),
            Self::Slurm => slurm_poll(target, handle, console_offset, cancelling),
        }
    }

    pub fn cancel(self, target: &RemoteTarget, handle: &LaunchHandle) -> Result<()> {
        match self {
            Self::Direct => direct_cancel(target),
            Self::Slurm => slurm_cancel(target, handle),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchHandle {
    pub id: String,
    pub cluster: Option<String>,
}

impl LaunchHandle {
    pub fn direct(id: String) -> Self {
        Self { id, cluster: None }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteJobPhase {
    Queued,
    Starting,
    Running,
    Completing,
    Cancelling,
    Succeeded,
    Failed,
    Cancelled,
    Unknown,
    Lost,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConsoleChunk {
    pub text: String,
    pub next_offset: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LauncherObservation {
    pub phase: RemoteJobPhase,
    pub scheduler_state: Option<String>,
    pub reason: Option<String>,
    pub exit_code: Option<i32>,
    pub console: ConsoleChunk,
}

pub fn retrieve_outcome(target: &RemoteTarget, working_dir: &Path) -> Result<Vec<u8>> {
    super::sync_down(
        target,
        working_dir,
        &[OUTCOME_FILE],
        &[CONSOLE_FILE, EXIT_FILE],
    )?;
    let path = working_dir.join(OUTCOME_FILE);
    std::fs::read(&path).with_context(|| format!("read {}", path.display()))
}

fn write_run_sh(working_dir: &Path, worker_path: &str, prelude: &[String]) -> Result<()> {
    let path = working_dir.join(RUN_SCRIPT);
    std::fs::write(&path, render_run_sh(worker_path, prelude))
        .with_context(|| format!("write {}", path.display()))
}

fn render_run_sh(worker_path: &str, prelude: &[String]) -> String {
    let mut script = String::from("#!/bin/sh\nset +e\n(\nset -e\n");
    for line in prelude {
        script.push_str(line);
        script.push('\n');
    }
    script.push_str(&format!(
        "{worker_path} exec {REQUEST_FILE} {OUTCOME_FILE}\n)\ncode=$?\nprintf '%s\\n' \"$code\" > {EXIT_FILE}.tmp.$$ && mv {EXIT_FILE}.tmp.$$ {EXIT_FILE}\nexit \"$code\"\n"
    ));
    script
}

fn render_slurm_script() -> &'static str {
    "#!/bin/sh\nexec sh run.sh\n"
}

fn direct_submit(target: &RemoteTarget) -> Result<LaunchHandle> {
    let body = format!("echo $$ > {PGID_FILE}; sh {RUN_SCRIPT} >> {CONSOLE_FILE} 2>&1");
    let launch = format!(
        "mkdir -p {dir} && cd {dir} && : > {CONSOLE_FILE} && {{ setsid sh -c {body} </dev/null >/dev/null 2>&1 & }}",
        dir = target.remote_dir,
        body = super::single_quote_wrap(&body),
    );
    let result = process::run(super::ssh_config(
        target,
        &launch,
        Some(Duration::from_secs(30)),
    ))
    .context("failed to launch the remote job over SSH")?;
    if !result.success() {
        bail!(
            "remote launch failed (exit {}): {}",
            result.exit_code,
            result.stderr.trim()
        );
    }
    Ok(LaunchHandle::direct(read_pgid(target).unwrap_or_default()))
}

fn read_pgid(target: &RemoteTarget) -> Option<String> {
    let script = format!(
        "cd {dir} 2>/dev/null || exit 0; i=0; while [ $i -lt 10 ] && [ ! -s {PGID_FILE} ]; do sleep 0.2; i=$((i+1)); done; cat {PGID_FILE} 2>/dev/null",
        dir = target.remote_dir,
    );
    let result = process::run(super::ssh_config(
        target,
        &script,
        Some(Duration::from_secs(10)),
    ))
    .ok()?;
    let pgid = result.stdout.trim();
    (valid_job_id(pgid)).then(|| pgid.to_string())
}

fn direct_poll(
    target: &RemoteTarget,
    console_offset: u64,
    cancelling: bool,
) -> Result<LauncherObservation> {
    let script = format!(
        "cd {dir} 2>/dev/null || {{ echo 'SL_DIRECT LOST'; echo 'SL_CONSOLE 0 0'; exit 0; }}; \
         SIZE=$(wc -c < {CONSOLE_FILE} 2>/dev/null || echo 0); START={offset}; \
         if [ \"$START\" -gt \"$SIZE\" ]; then START=0; fi; \
         if [ -f {EXIT_FILE} ]; then CODE=$(cat {EXIT_FILE} 2>/dev/null); \
         if [ \"$CODE\" = 0 ] && [ -s {OUTCOME_FILE} ]; then echo 'SL_DIRECT SUCCEEDED 0'; \
         elif [ \"$CODE\" = 0 ]; then echo 'SL_DIRECT FAILED 0 missing_outcome'; \
         else printf 'SL_DIRECT FAILED %s\\n' \"$CODE\"; fi; \
         else PGID=$(cat {PGID_FILE} 2>/dev/null); \
         if [ -n \"$PGID\" ] && kill -0 -\"$PGID\" 2>/dev/null; then echo 'SL_DIRECT RUNNING'; else echo 'SL_DIRECT LOST'; fi; fi; \
         printf 'SL_CONSOLE %s %s\\n' \"$START\" \"$SIZE\"; \
         if [ \"$SIZE\" -gt \"$START\" ]; then tail -c +$((START+1)) {CONSOLE_FILE} 2>/dev/null; fi",
        dir = target.remote_dir,
        offset = console_offset,
    );
    let result = process::run(super::ssh_config(
        target,
        &script,
        Some(Duration::from_secs(30)),
    ))
    .context("failed to poll the remote job over SSH")?;
    if !result.success() {
        bail!("remote poll failed (exit {})", result.exit_code);
    }
    parse_direct_observation(&result.stdout, cancelling)
}

fn parse_direct_observation(output: &str, cancelling: bool) -> Result<LauncherObservation> {
    let (status, console) = split_poll_output(output)?;
    let mut fields = status.split_whitespace();
    if fields.next() != Some("SL_DIRECT") {
        bail!("invalid Direct status response");
    }
    let token = fields.next().unwrap_or("LOST");
    let exit_code = fields.next().and_then(|value| value.parse().ok());
    let mut reason = fields.next().map(str::to_string);
    let phase = match token {
        "RUNNING" if cancelling => RemoteJobPhase::Cancelling,
        "RUNNING" => RemoteJobPhase::Running,
        "SUCCEEDED" => RemoteJobPhase::Succeeded,
        "FAILED" => RemoteJobPhase::Failed,
        // A killed process group never writes `.exit`, so a cancelled Direct job
        // is indistinguishable from a lost one except by intent.
        "LOST" if cancelling => RemoteJobPhase::Cancelled,
        "LOST" => RemoteJobPhase::Lost,
        _ => {
            reason = Some("unrecognized Direct status".to_string());
            RemoteJobPhase::Lost
        }
    };
    Ok(LauncherObservation {
        phase,
        scheduler_state: None,
        reason,
        exit_code,
        console,
    })
}

fn direct_cancel(target: &RemoteTarget) -> Result<()> {
    let script = format!(
        "cd {dir} 2>/dev/null || exit 0; PGID=$(cat {PGID_FILE} 2>/dev/null); \
         if [ -n \"$PGID\" ]; then kill -TERM -- -$PGID 2>/dev/null; sleep 1; kill -KILL -- -$PGID 2>/dev/null; fi; true",
        dir = target.remote_dir,
    );
    run_scheduler_command(
        target,
        &script,
        Duration::from_secs(20),
        "cancel the remote job",
    )?;
    Ok(())
}

fn slurm_submit(
    target: &RemoteTarget,
    resources: &JobResources,
    profile: &SlurmProfile,
) -> Result<LaunchHandle> {
    profile.validate()?;
    let command = render_sbatch_command(target, resources, profile)?;
    let result = process::run(super::ssh_config(
        target,
        &command,
        Some(Duration::from_secs(30)),
    ))
    .context("failed to submit the Slurm job over SSH")?;
    if !result.success() {
        bail!(
            "Slurm submission failed (exit {}): {}",
            result.exit_code,
            result.stderr.trim()
        );
    }
    parse_sbatch_output(&result.stdout)
}

fn render_sbatch_command(
    target: &RemoteTarget,
    resources: &JobResources,
    profile: &SlurmProfile,
) -> Result<String> {
    resources.validate()?;
    profile.validate()?;
    let run_id = target.remote_dir.rsplit('/').next().unwrap_or("run");
    let short_id: String = run_id.chars().take(8).collect();
    let mut args = vec![
        "sbatch".to_string(),
        "--parsable".to_string(),
        format!("--job-name=silicolab-{short_id}"),
        "--nodes=1".to_string(),
        "--ntasks=1".to_string(),
    ];
    if let Some(cpus) = resources.cpus_per_task {
        args.push(format!("--cpus-per-task={cpus}"));
    }
    if let Some(memory) = resources.memory_mib {
        args.push(format!("--mem={memory}M"));
    }
    if let Some(seconds) = resources.walltime_seconds {
        args.push(format!("--time={}", render_walltime(seconds)));
    }
    for (name, value) in [
        ("partition", profile.partition.as_deref()),
        ("account", profile.account.as_deref()),
        ("qos", profile.qos.as_deref()),
        ("reservation", profile.reservation.as_deref()),
        ("constraint", profile.constraint.as_deref()),
    ] {
        if let Some(value) = value {
            args.push(format!("--{name}={value}"));
        }
    }
    if let Some(gpu) = render_gpu_argument(&resources.gpu, &profile.gpu_syntax)? {
        args.push(gpu);
    }
    args.extend(profile.extra_args.iter().cloned());
    args.extend([
        "--chdir=$PWD".to_string(),
        format!("--output={CONSOLE_FILE}"),
        format!("--error={CONSOLE_FILE}"),
        "--open-mode=truncate".to_string(),
        SLURM_SCRIPT.to_string(),
    ]);
    let rendered = args
        .iter()
        .map(|arg| {
            if arg == "--chdir=$PWD" {
                "--chdir=\"$PWD\"".to_string()
            } else {
                super::sh_quote(arg)
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    Ok(format!(
        "cd {} && {}{}",
        target.remote_dir,
        target.scheduler_prefix(),
        rendered
    ))
}

fn render_gpu_argument(gpu: &GpuRequest, syntax: &SlurmGpuSyntax) -> Result<Option<String>> {
    gpu.validate()?;
    let (gpu_type, count) = match gpu {
        GpuRequest::None => return Ok(None),
        GpuRequest::Any { count } => (None, *count),
        GpuRequest::Typed { gpu_type, count } => (Some(gpu_type.as_str()), *count),
    };
    let value = match syntax {
        SlurmGpuSyntax::Gres { resource_name } => match gpu_type {
            Some(gpu_type) => format!("--gres={resource_name}:{gpu_type}:{count}"),
            None => format!("--gres={resource_name}:{count}"),
        },
        SlurmGpuSyntax::Gpus => match gpu_type {
            Some(gpu_type) => format!("--gpus={gpu_type}:{count}"),
            None => format!("--gpus={count}"),
        },
        SlurmGpuSyntax::CustomTemplate { argument } => argument
            .replace("{count}", &count.to_string())
            .replace("{type}", gpu_type.unwrap_or("")),
    };
    Ok(Some(value))
}

fn render_walltime(seconds: u64) -> String {
    let days = seconds / 86_400;
    let hours = seconds % 86_400 / 3_600;
    let minutes = seconds % 3_600 / 60;
    let seconds = seconds % 60;
    if days > 0 {
        format!("{days}-{hours:02}:{minutes:02}:{seconds:02}")
    } else {
        format!("{hours:02}:{minutes:02}:{seconds:02}")
    }
}

fn parse_sbatch_output(output: &str) -> Result<LaunchHandle> {
    let line = output.trim();
    let mut parts = line.split(';');
    let id = parts.next().unwrap_or_default();
    let cluster = parts.next();
    if !valid_job_id(id) || parts.next().is_some() {
        bail!("invalid `sbatch --parsable` response `{line}`");
    }
    if cluster.is_some_and(|value| !valid_cluster_name(value)) {
        bail!("invalid cluster name in `sbatch --parsable` response");
    }
    Ok(LaunchHandle {
        id: id.to_string(),
        cluster: cluster.map(str::to_string),
    })
}

fn valid_job_id(value: &str) -> bool {
    !value.is_empty() && value.chars().all(|ch| ch.is_ascii_digit())
}

fn valid_cluster_name(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'))
}

fn slurm_poll(
    target: &RemoteTarget,
    handle: &LaunchHandle,
    console_offset: u64,
    cancelling: bool,
) -> Result<LauncherObservation> {
    if !valid_job_id(&handle.id) {
        bail!("invalid Slurm JobID");
    }
    let cluster_arg = handle
        .cluster
        .as_ref()
        .map(|name| format!(" --clusters={}", super::sh_quote(name)))
        .unwrap_or_default();
    let prefix = target.scheduler_prefix();
    let script = format!(
        "cd {dir} 2>/dev/null || {{ echo 'SLURM_LOST'; echo 'SL_CONSOLE 0 0'; exit 0; }}; \
         SIZE=$(wc -c < {CONSOLE_FILE} 2>/dev/null || echo 0); START={offset}; \
         if [ \"$START\" -gt \"$SIZE\" ]; then START=0; fi; \
         if [ -f {EXIT_FILE} ]; then CODE=$(cat {EXIT_FILE} 2>/dev/null); \
         if [ \"$CODE\" = 0 ] && [ -s {OUTCOME_FILE} ]; then echo 'SLURM_MARKER COMPLETED 0'; \
         elif [ \"$CODE\" = 0 ]; then echo 'SLURM_MARKER FAILED 0 missing_outcome'; \
         else printf 'SLURM_MARKER FAILED %s\\n' \"$CODE\"; fi; \
         else OUT=$({prefix}squeue -h -j {id}{cluster_arg} -o '%T|%r' 2>/dev/null); \
         if [ -n \"$OUT\" ]; then printf 'SLURM_SQUEUE %s\\n' \"$OUT\"; \
         else OUT=$({prefix}sacct -X -n -P -j {id}{cluster_arg} --format=JobIDRaw,State,ExitCode,Reason 2>/dev/null | head -n 1); \
         if [ -n \"$OUT\" ]; then printf 'SLURM_SACCT %s\\n' \"$OUT\"; \
         else OUT=$({prefix}scontrol show job -o {id} 2>/dev/null); \
         if [ -n \"$OUT\" ]; then printf 'SLURM_SCONTROL %s\\n' \"$OUT\"; else echo 'SLURM_UNKNOWN'; fi; fi; fi; fi; \
         printf 'SL_CONSOLE %s %s\\n' \"$START\" \"$SIZE\"; \
         if [ \"$SIZE\" -gt \"$START\" ]; then tail -c +$((START+1)) {CONSOLE_FILE} 2>/dev/null; fi",
        dir = target.remote_dir,
        offset = console_offset,
        id = handle.id,
    );
    let result = process::run(super::ssh_config(
        target,
        &script,
        Some(Duration::from_secs(30)),
    ))
    .context("failed to poll Slurm over SSH")?;
    if !result.success() {
        bail!("Slurm poll failed (exit {})", result.exit_code);
    }
    parse_slurm_observation(&result.stdout, &handle.id, cancelling)
}

fn parse_slurm_observation(
    output: &str,
    job_id: &str,
    cancelling: bool,
) -> Result<LauncherObservation> {
    let (status, console) = split_poll_output(output)?;
    let mut observation = LauncherObservation {
        phase: RemoteJobPhase::Unknown,
        scheduler_state: None,
        reason: None,
        exit_code: None,
        console,
    };
    if status == "SLURM_LOST" {
        observation.phase = RemoteJobPhase::Lost;
        return Ok(observation);
    }
    if status == "SLURM_UNKNOWN" {
        return Ok(observation);
    }
    if let Some(rest) = status.strip_prefix("SLURM_MARKER ") {
        let mut fields = rest.split_whitespace();
        let state = fields.next().unwrap_or("FAILED");
        observation.scheduler_state = Some(state.to_string());
        observation.exit_code = fields.next().and_then(|value| value.parse().ok());
        observation.reason = fields.next().map(str::to_string);
        observation.phase = if state == "COMPLETED" {
            RemoteJobPhase::Succeeded
        } else {
            RemoteJobPhase::Failed
        };
        return Ok(observation);
    }
    let (state, reason, exit_code) = if let Some(rest) = status.strip_prefix("SLURM_SQUEUE ") {
        parse_squeue_line(rest)?
    } else if let Some(rest) = status.strip_prefix("SLURM_SACCT ") {
        parse_sacct_line(rest, job_id)?
    } else if let Some(rest) = status.strip_prefix("SLURM_SCONTROL ") {
        parse_scontrol_line(rest)?
    } else {
        bail!("invalid Slurm status response");
    };
    observation.phase = map_slurm_state(&state, cancelling, exit_code);
    observation.scheduler_state = Some(state);
    observation.reason = reason;
    observation.exit_code = exit_code;
    Ok(observation)
}

fn split_poll_output(output: &str) -> Result<(&str, ConsoleChunk)> {
    let mut parts = output.splitn(3, '\n');
    let status = parts.next().unwrap_or_default().trim();
    let meta = parts.next().unwrap_or_default().trim();
    let text = parts.next().unwrap_or_default().to_string();
    let mut fields = meta.split_whitespace();
    if fields.next() != Some("SL_CONSOLE") {
        bail!("remote poll response omitted console metadata");
    }
    let _start: u64 = fields
        .next()
        .ok_or_else(|| anyhow::anyhow!("console offset is missing"))?
        .parse()
        .context("invalid console offset")?;
    let next_offset = fields
        .next()
        .ok_or_else(|| anyhow::anyhow!("console size is missing"))?
        .parse()
        .context("invalid console size")?;
    Ok((status, ConsoleChunk { text, next_offset }))
}

fn parse_squeue_line(line: &str) -> Result<(String, Option<String>, Option<i32>)> {
    let mut fields = line.trim().splitn(2, '|');
    let state = fields.next().unwrap_or_default().trim();
    if state.is_empty() {
        bail!("Slurm state is missing");
    }
    let reason = fields
        .next()
        .map(str::trim)
        .filter(|value| !value.is_empty() && *value != "None")
        .map(str::to_string);
    Ok((normalize_slurm_state(state), reason, None))
}

fn parse_sacct_line(line: &str, job_id: &str) -> Result<(String, Option<String>, Option<i32>)> {
    let fields: Vec<_> = line.trim().split('|').collect();
    if fields.len() < 4 || fields[0] != job_id {
        bail!("invalid sacct response");
    }
    let state = normalize_slurm_state(fields[1]);
    let exit_code = parse_slurm_exit_code(fields[2]);
    let reason = (!fields[3].is_empty() && fields[3] != "None").then(|| fields[3].to_string());
    Ok((state, reason, exit_code))
}

fn parse_scontrol_line(line: &str) -> Result<(String, Option<String>, Option<i32>)> {
    let values = line
        .split_whitespace()
        .filter_map(|field| field.split_once('='))
        .collect::<std::collections::HashMap<_, _>>();
    let state = values
        .get("JobState")
        .map(|value| normalize_slurm_state(value))
        .ok_or_else(|| anyhow::anyhow!("scontrol response omitted JobState"))?;
    let reason = values
        .get("Reason")
        .filter(|value| !value.is_empty() && **value != "None")
        .map(|value| (*value).to_string());
    let exit_code = values
        .get("ExitCode")
        .and_then(|value| parse_slurm_exit_code(value));
    Ok((state, reason, exit_code))
}

fn normalize_slurm_state(state: &str) -> String {
    state
        .trim()
        .trim_end_matches(['+', '*'])
        .split_whitespace()
        .next()
        .unwrap_or("UNKNOWN")
        .to_ascii_uppercase()
}

fn parse_slurm_exit_code(value: &str) -> Option<i32> {
    value.split(':').next()?.parse().ok()
}

fn map_slurm_state(state: &str, cancelling: bool, exit_code: Option<i32>) -> RemoteJobPhase {
    match state {
        "PENDING" | "REQUEUED" | "REQUEUE_FED" | "REQUEUE_HOLD" => {
            if cancelling {
                RemoteJobPhase::Cancelling
            } else {
                RemoteJobPhase::Queued
            }
        }
        "CONFIGURING" => RemoteJobPhase::Starting,
        "RUNNING" | "SUSPENDED" | "STOPPED" | "RESIZING" => {
            if cancelling {
                RemoteJobPhase::Cancelling
            } else {
                RemoteJobPhase::Running
            }
        }
        "COMPLETING" | "STAGE_OUT" | "SIGNALING" => {
            if cancelling {
                RemoteJobPhase::Cancelling
            } else {
                RemoteJobPhase::Completing
            }
        }
        "COMPLETED" if exit_code.unwrap_or(0) == 0 => RemoteJobPhase::Succeeded,
        "CANCELLED" => RemoteJobPhase::Cancelled,
        "BOOT_FAIL" | "DEADLINE" | "FAILED" | "NODE_FAIL" | "OUT_OF_MEMORY" | "PREEMPTED"
        | "SPECIAL_EXIT" | "TIMEOUT" | "COMPLETED" => RemoteJobPhase::Failed,
        _ => RemoteJobPhase::Unknown,
    }
}

fn slurm_cancel(target: &RemoteTarget, handle: &LaunchHandle) -> Result<()> {
    if !valid_job_id(&handle.id) {
        bail!("invalid Slurm JobID");
    }
    let mut args = vec!["scancel".to_string(), handle.id.clone()];
    if let Some(cluster) = &handle.cluster {
        if !valid_cluster_name(cluster) {
            bail!("invalid Slurm cluster name");
        }
        args.push(format!("--clusters={cluster}"));
    }
    let command = format!(
        "{}{}",
        target.scheduler_prefix(),
        args.iter()
            .map(|arg| super::sh_quote(arg))
            .collect::<Vec<_>>()
            .join(" ")
    );
    run_scheduler_command(
        target,
        &command,
        Duration::from_secs(20),
        "cancel the Slurm job",
    )?;
    Ok(())
}

fn run_scheduler_command(
    target: &RemoteTarget,
    script: &str,
    timeout: Duration,
    context: &str,
) -> Result<String> {
    let result = process::run(super::ssh_config(target, script, Some(timeout)))
        .with_context(|| format!("failed to {context} over SSH"))?;
    if !result.success() {
        let detail = if result.stderr.trim().is_empty() {
            result.stdout.trim()
        } else {
            result.stderr.trim()
        };
        bail!("{context} failed (exit {}): {detail}", result.exit_code);
    }
    Ok(result.stdout)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlurmDetection {
    pub version: String,
    pub sacct_available: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SlurmCapabilities {
    pub partitions: Vec<String>,
    pub gpu_types: Vec<String>,
    pub features: Vec<String>,
}

pub fn detect_slurm(target: &RemoteTarget) -> Result<SlurmDetection> {
    // `sacct` is on PATH on every Slurm install; it is only *usable* when the
    // cluster runs accounting storage. Probe the query, not the binary.
    let script = format!(
        "{}for cmd in sbatch squeue scancel scontrol; do command -v \"$cmd\" >/dev/null 2>&1 || {{ echo \"missing:$cmd\"; exit 12; }}; done; sbatch --version; if sacct -n -X -j 1 >/dev/null 2>&1; then echo SACCT=yes; else echo SACCT=no; fi",
        target.scheduler_prefix()
    );
    let output = run_scheduler_command(target, &script, Duration::from_secs(30), "detect Slurm")?;
    if let Some(missing) = output
        .lines()
        .find_map(|line| line.strip_prefix("missing:"))
    {
        bail!("required Slurm command `{missing}` is unavailable");
    }
    let version = output
        .lines()
        .find(|line| line.to_ascii_lowercase().contains("slurm"))
        .unwrap_or("Slurm")
        .trim()
        .to_string();
    Ok(SlurmDetection {
        version,
        sacct_available: output.lines().any(|line| line.trim() == "SACCT=yes"),
    })
}

pub fn probe_slurm_capabilities(target: &RemoteTarget) -> Result<SlurmCapabilities> {
    let script = format!("{}sinfo -h -o '%P|%G|%f|%t'", target.scheduler_prefix());
    let output = run_scheduler_command(
        target,
        &script,
        Duration::from_secs(30),
        "query Slurm capabilities",
    )?;
    Ok(parse_sinfo(&output))
}

fn parse_sinfo(output: &str) -> SlurmCapabilities {
    let mut capabilities = SlurmCapabilities::default();
    for line in output.lines() {
        let fields: Vec<_> = line.split('|').collect();
        if fields.len() < 3 {
            continue;
        }
        let partition = fields[0].trim().trim_end_matches('*');
        if !partition.is_empty() && !capabilities.partitions.iter().any(|item| item == partition) {
            capabilities.partitions.push(partition.to_string());
        }
        for gres in fields[1].split(',') {
            let parts: Vec<_> = gres.trim().split(':').collect();
            if parts.first() == Some(&"gpu") && parts.len() >= 3 {
                let gpu_type = parts[1].to_string();
                if !capabilities.gpu_types.contains(&gpu_type) {
                    capabilities.gpu_types.push(gpu_type);
                }
            }
        }
        for feature in fields[2]
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty() && *value != "(null)")
        {
            if !capabilities.features.iter().any(|item| item == feature) {
                capabilities.features.push(feature.to_string());
            }
        }
    }
    capabilities.partitions.sort();
    capabilities.gpu_types.sort();
    capabilities.features.sort();
    capabilities
}

#[cfg(test)]
mod tests;
