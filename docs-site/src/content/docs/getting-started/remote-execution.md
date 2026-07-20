---
title: Remote execution
description: Run compute jobs on a remote Linux host through Direct SSH or Slurm.
sidebar:
  order: 6
---

Quantum chemistry, molecular docking, and MD/GROMACS jobs can run on a remote
x86-64 Linux host while the GUI remains on your computer. SilicoLab uses the
operating system's `ssh` and `scp` clients and deploys a checksum-verified,
version-matched worker on first use, caching it under `~/.silicolab/workers` so
later deployments — including to new hosts — work without network access.

## Set up SSH

Open **Settings > Compute > Compute targets**, add or expand the remote target,
and enter its host address, SSH user, port, and work directory. Then select
**Set up passwordless login**. SilicoLab creates a dedicated key under
`~/.silicolab/keys` and keeps strict host-key verification enabled. Run the
displayed authorization command once on the remote host, then select **Verify**.

The work directory defaults to `~/.silicolab`. It must be visible and writable
on every node that may execute a job. On a cluster this normally means a shared
home, project, or scratch filesystem. Select **Test scheduler** after configuring
Slurm; the test submits a real short job and verifies worker visibility from the
allocated node.

**Setup commands** run inside the allocated job before the worker. Use them for
commands such as `module load gromacs` or CUDA setup. **Scheduler
setup commands** run on the login node before `sbatch`, `squeue`, `scontrol`, and
`scancel`; use them only when Slurm commands are not already on the
non-interactive SSH `PATH`.

## Direct SSH

Choose **Direct SSH** for a dedicated workstation or bare compute node. The
worker runs as a detached process group. CPU requests limit the worker thread
pool; memory and walltime remain advisory because no scheduler enforces them.

## Slurm

Choose **Slurm** and configure the fields required by the cluster:

- **Partition** selects a queue, such as `debug` or `gpu`.
- **Account** identifies the allocation charged for the job.
- **QOS** selects a site-defined quality-of-service policy.
- **Reservation** and **Constraint** are optional advanced selectors.
- Default CPU, memory, and walltime values apply when a task does not override
  them.

Select **Detect Slurm** to verify `sbatch`, `squeue`, `scancel`, and `scontrol`.
`sacct` is optional; when it is unavailable, SilicoLab uses `scontrol` for
terminal history. **Refresh cluster** loads partition, GPU-type, and feature
suggestions. These values are hints and do not reserve currently idle hardware.

GRES is the default GPU dialect. A task can request:

- **No GPU**;
- **Any available GPU** with a count; or
- **Specific type** with a scheduler GPU type such as `a100` and a count.

SilicoLab translates these requests to `--gres=gpu[:type]:count`. Select the
`--gpus` dialect only when the cluster administrator recommends it. A site whose
GPU request looks like neither can use **Custom**, a template argument with
`{count}` and an optional `{type}` placeholder — for example
`--gres=accel:{type}:{count}`. Slurm, not SilicoLab, chooses physical device
indices and exposes the allocation to the job.

## Run and monitor jobs

Choose the host under **Run on** in a task panel, then set CPU, memory, walltime,
and GPU intent. **Activity** shows each job's current status. For queued Slurm
jobs it also shows scheduler reasons such as `Priority`, `Resources`,
`InvalidAccount`, or `InvalidQOS`.

Use **Refresh Remote** to retrieve the latest state and new job logs. The newest
lines appear in **Activity**; select **Open in Output** when you need the job's
full log. Closing SilicoLab does not stop a remote job. Reopen the project and
refresh to continue monitoring or retrieve its result. The scheduler and remote
directory captured at submission remain authoritative even if the host settings
are edited later.

Select **Cancel** for a queued or running job. Activity continues to show
cancellation in progress until Slurm confirms it; repeated requests are safe.
Remote scratch can be removed only after a terminal state is confirmed.

Login-node CPU and GPU utilization is not shown as cluster utilization for a
Slurm target. Allocation state and pending reason belong in **Activity**.

## Troubleshooting

- **Invalid account, QOS, or partition:** copy the exact values supplied by the
  cluster administrator and refresh the job to see Slurm's pending reason.
- **Worker is not visible on the compute node:** move the work root to a shared
  filesystem and run **Test scheduler** again.
- **No terminal history:** `sacct` may be disabled. SilicoLab automatically uses
  `scontrol`; controller retention still determines how long old jobs remain
  observable.
- **Typed GPU remains pending:** confirm the type spelling under **Refresh
  cluster** and check that the selected partition contains that GRES type.
- **GROMACS is missing:** add the appropriate module or environment command to
  **Setup commands**, then verify GROMACS under that target's **Program** field.
- **ORCA is not configured:** enter the ORCA executable path for that remote
  host under **Settings > Compute > Compute targets**, then select **Verify**.
- **Worker download fails (offline, proxy, or GitHub rate limit):** the
  version-matched worker is fetched from GitHub the first time you deploy a given
  SilicoLab version, then cached under `~/.silicolab/workers` for reuse. To use
  remote compute offline, run one remote job while online after each update to warm
  the cache. A cached worker is reused without any network request; only the first
  fetch for a new version needs GitHub.
