# SilicoLab

Computational environment for chemistry, biology & materials research.

![SilicoLab screenshot](docs/images/main-window.png)

## Features

- Interactive 3D visualization and editing of molecular and crystal structures
- Force-field geometry optimization
- Quantum chemistry calculations
- Guided molecular dynamics setup and execution (powered by GROMACS)
- Reticular structure builder â€” assemble COFs and MOFs from building blocks
- One scripting language for everything: the same scripts run in the GUI
  console and headless on the CLI, making workflows easy to automate and
  agent-friendly

## Installation

### Prebuilt Executables

Prebuilt executables can be downloaded from GitHub Releases.

### Build from Source

Install the Rust toolchain, then build the release executable:

```sh
cargo build --release
```

The built binary is written to `target/release/` (`silicolab` on Linux/macOS,
`silicolab.exe` on Windows).

## Usage

Run with no arguments to launch the GUI:

```sh
silicolab
```

Pass a script path to run it headless from the command line:

```sh
silicolab workflow.sls
```

The same scripts also run interactively in the GUI console. Scripting
documentation is in progress.

## External Dependencies

### GROMACS (required for molecular dynamics)

Molecular dynamics simulations require [GROMACS](https://www.gromacs.org/) to be installed separately.
GPU acceleration is **strongly recommended** â€” running MD on CPU alone is technically possible but prohibitively slow for any non-trivial system.

- **Windows:** Install GROMACS inside [WSL](https://learn.microsoft.com/en-us/windows/wsl/install) (`sudo apt install gromacs`). For GPU support, compile from source with CUDA inside WSL.
- **Linux:** `sudo apt install gromacs` for a quick start; compile from source with CUDA/ROCm for GPU acceleration.
- **macOS:** `brew install gromacs`. Note that GPU acceleration is not supported on Apple hardware, so MD performance will be limited.

### Remote execution over SSH (optional)

Heavy MD can be offloaded to a remote machine (an HPC login node, a GPU box) while
the GUI stays on your laptop. SilicoLab drives the OS OpenSSH client (`ssh`/`scp`)
as subprocesses â€” no extra dependency to install (the OpenSSH client ships with
macOS/Linux and is an on-by-default optional feature on Windows 11; enable it under
*Settings â†’ Apps â†’ Optional features â†’ OpenSSH Client* if it is missing).

**Set up a host** in *Settings â†’ Engines â†’ Remote Hosts*:

1. **Add host** â€” give it a label, hostname/IP, username, and (optionally) a port
   and a remote work directory (defaults to `~/.silicolab`). Under *Setup commands*
   put whatever a fresh, non-interactive SSH shell needs to make `gmx` runnable â€”
   e.g. `module load gromacs` or `source /opt/gromacs/bin/GMXRC` â€” one per line.
2. **Set up passwordless login** â€” SilicoLab generates a dedicated key
   (`~/.silicolab/keys/id_silicolab_ed25519`, never your own keys) and shows a
   one-line command to run once on the host (paste it into a terminal, or type
   `! <command>` in the SilicoLab prompt). Click **Verify** to confirm. Passwordless
   (key-based) login is required so unattended jobs never block on a password.
3. **Detect GROMACS** â€” probes the host for `gmx` and records its version.

**Run remotely:** in the Run MD or Build MD System panel, set **Compute** to your
host. SilicoLab stages the inputs up, launches each `gmx` step detached so a dropped
connection can't kill it, streams the live log back, and stages results down â€” the
result (structure, energies, trajectory) appears exactly as for a local run. Press
**Esc** to cancel (it kills the remote job too).

v1 limitations: a remote run occupies the single engine-job slot while active;
closing the app leaves an in-flight remote job running (a `remote_run.json` record
is written into the local run directory) but does not auto-reattach to it; remote
scratch directories under `<work_root>/runs/<run-id>` are not garbage-collected
automatically.

## License

SilicoLab is available under either:

- [GPL-3.0-or-later](LICENSES/GPL-3.0-or-later.txt), or
- a separate commercial license granted in writing by the SilicoLab copyright holders.

If you do not have a signed commercial license agreement, your rights are under GPL-3.0-or-later. The repository records this dual-license structure with REUSE/SPDX metadata in [REUSE.toml](REUSE.toml): GPL-3.0-or-later OR LicenseRef-SilicoLab-Commercial.

Third-party components remain under their own licenses; see [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md).

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in this work by you is submitted under GPL-3.0-or-later and also grants the SilicoLab copyright holders the right to offer that contribution under separate commercial licenses.
