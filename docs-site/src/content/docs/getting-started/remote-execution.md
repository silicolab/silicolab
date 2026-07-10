---
title: Remote execution
description: Offload heavy compute jobs to a remote Linux host over SSH.
sidebar:
  order: 4
---

Heavy compute can be offloaded to a remote Linux machine, such as an HPC login
node or GPU workstation, while the GUI stays on your laptop.

Quantum chemistry, molecular docking, and MD/GROMACS can all run remotely.
SilicoLab drives the OS OpenSSH client (`ssh` and `scp`) as subprocesses. The
OpenSSH client ships with macOS and Linux. On Windows 11, enable it under
**Settings > Apps > Optional features > OpenSSH Client** if it is missing.

On first use, SilicoLab deploys a small self-contained worker to the host. The
worker is pinned to the app version and verified against its published checksum
before it runs.

Testing remote changes from source is a contributor workflow; follow the
[remote execution development guide](https://github.com/silicolab/silicolab/blob/main/docs/developing-remote-execution.md).
Released builds always use the version-pinned, checksum-verified worker described
above.

## Set up a host

Open **Settings > Engines > Remote Hosts**.

1. Select **Add host** and enter a label, hostname or IP address, username, and
   optionally a port and remote work directory. The default work directory is
   `~/.silicolab`. A custom work directory must be an absolute Linux path or
   start with `~/`.
2. Under **Setup commands**, enter the commands a fresh non-interactive SSH
   shell needs before engine commands are available. For example, use
   `module load gromacs` or `source /opt/gromacs/bin/GMXRC` to make `gmx`
   runnable. Enter one command per line.
3. Select **Set up passwordless login**. SilicoLab generates a dedicated key at
   `~/.silicolab/keys/id_silicolab_ed25519` and shows a one-line command to run
   once on the host. This key is separate from your personal SSH keys.
4. Select **Verify** to confirm that key-based login works. Passwordless login
   is required so unattended jobs never block on a password prompt.
5. Select **Detect GROMACS** if you plan to run MD. This probes the host for
   `gmx` and records the detected version. Quantum chemistry and docking run
   inside the deployed worker, so they do not need a host-side GROMACS install.

## Run remotely

Task panels for **Run MD**, **Build MD System**, **QM**, and **Molecular
Docking** include a **Run on** selector. Pick the remote host there. In **Build
MD System**, the selector applies to the GROMACS build step; the built-in
geometry build always runs locally.

New panels start from the **Default compute target** configured in
**Settings > Engines > Remote Hosts**, and you can change the target per run.

SilicoLab stages inputs up, runs the job, streams the live log back, and stages
results down. The result appears in the project the same way as a local run.
For GROMACS jobs, each `gmx` step is launched detached so a dropped SSH
connection does not kill the calculation.

Press **Esc** to cancel a remote run. SilicoLab also stops the remote job.

## Current limitations

- A remote run occupies the single engine-job slot while active.
- Closing the app leaves an in-flight remote job running. SilicoLab writes a
  `remote_run.json` record into the local run directory, but it does not
  auto-reattach to that job yet.
- Remote scratch directories under `<work_root>/runs/<run-id>` are not
  garbage-collected automatically.
