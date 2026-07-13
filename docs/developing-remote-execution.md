# Developing remote execution

Use this guide when a change must run through the remote worker before it has
been published in a SilicoLab release. The standard entry point is:

```text
cargo xtask remote-dev
```

It cross-compiles the worker from the current checkout, validates the resulting
Linux executable, and launches the application in development-worker mode. The
remote host receives a binary; it does not need the source tree or a development
toolchain.

For released behavior and host setup, see the end-user
[remote execution guide](../docs-site/src/content/docs/getting-started/remote-execution.md).

## Production and development artifacts

Remote deployment has two deliberately separate artifact sources:

| Application build | Worker source | Deployment identity |
|---|---|---|
| Ordinary or official build | The `silicolab-compute-x86_64-unknown-linux-musl` asset from the GitHub Release whose tag exactly matches the application package version, plus that release's published SHA-256 | The package version |
| Build with the contributor-only `dev-worker` feature | A validated local `x86_64-unknown-linux-musl` worker | `dev:<full SHA-256 of the binary>` |

Production remains fail-closed. It will not use an unverified download, a
different release tag, or a local development artifact. Development worker
resolution reads the selected local file without contacting GitHub Releases and
never falls back to a released worker.

`remote-dev` also suppresses the application's automatic startup release check,
so the development workflow makes no incidental GitHub Releases request.

Both sources use the same installation sequence: upload an identity-qualified
file, make it executable, check its reported package version, and only then
atomically update the stable `silicolab-compute` symlink.
The host's persisted `_worker` value is a deployment identity: production
deployments store the package version, while development deployments store the
`dev:<sha256>` form.

The identity is derived from the bytes that will actually run. Rebuilding
without changing the binary reuses the verified remote artifact; changing the
binary produces a new identity and filename. A matching cached identity is only
a fast-path hint: SilicoLab still checks that the remote executable exists and
runs, and redeploys it if that verification fails. Switching between a release
build and a development build also changes the identity, so neither can reuse
the other's worker accidentally.

Each job launches the verified identity-qualified path directly. The stable
symlink is published only after verification and does not decide which bytes an
already-submitted job runs, so concurrent checkouts cannot swap workers under
one another.

## Prerequisites

On the development machine, install:

- the repository's normal Rust toolchain through `rustup`;
- Cargo; and
- the operating system's OpenSSH client (`ssh`, `scp`, and `ssh-keygen`).

`cargo xtask` checks for the `x86_64-unknown-linux-musl` Rust standard-library
target and runs `rustup target add` when it is missing. The repository configures
that target to link with the Rust toolchain's `rust-lld` in self-contained mode.
The worker's dependencies are pure Rust, so no Docker, WSL, musl C compiler, or
cross-compilation SDK is required.

Platform notes:

- **Windows:** use a rustup-managed Rust installation. Windows 11 provides
  OpenSSH Client under **Settings > Apps > Optional features**. WSL is not part
  of the build path.
- **macOS:** the system OpenSSH client is sufficient. The worker still builds as
  a Linux executable; it is not run on the Mac.
- **Linux:** the system OpenSSH client is sufficient. The same cross-target
  command is used even when the development machine is already x86_64 Linux.

The remote machine must run **x86_64 Linux** and accept passwordless login
through SilicoLab's host-key-verified SSH configuration. ARM64/aarch64 hosts are
not supported by this worker. The remote machine needs no Rust toolchain,
container runtime, source checkout, compiler, or operating-system changes.
GROMACS jobs require a working `gmx` on the remote host, while ORCA-backed QM
jobs require the configured ORCA executable; built-in QM and docking do
not.

## Standard workflows

Launch the GUI with the current-source worker:

```text
cargo xtask remote-dev
```

Pass normal application arguments after `--`. For example, to run a script
headlessly:

```text
cargo xtask remote-dev -- path/to/workflow.sls --name value
```

The command performs these steps in order:

1. ensure the musl Rust target is installed;
2. run the release-profile build of `silicolab-compute` for that target;
3. validate that the output is an ELF64, little-endian, x86-64 executable; and
4. launch SilicoLab with `dev-worker` enabled and the artifact selected.

The default artifact is:

```text
target/x86_64-unknown-linux-musl/release/silicolab-compute
```

There is no `.exe` suffix, including when the development machine is Windows,
because the file targets Linux.

For an IDE pre-launch task or a build-only check, run:

```text
cargo xtask build-dev-worker
```

This performs the target check, release build, and ELF validation without
starting SilicoLab.

### Use an explicit local artifact

`SILICOLAB_DEV_WORKER` overrides the default artifact path when the
`dev-worker` feature is enabled. The file must satisfy the same size and ELF
validation as the default build. Relative paths are resolved from the process's
working directory, so prefer an absolute path in IDE settings.

PowerShell:

```powershell
$env:SILICOLAB_DEV_WORKER = 'C:\path\to\silicolab-compute'
cargo run --release --features dev-worker
```

macOS or Linux shell:

```sh
SILICOLAB_DEV_WORKER=/path/to/silicolab-compute \
  cargo run --release --features dev-worker
```

The override is contributor-only: builds without `dev-worker` neither inspect
nor honor it. Prefer `cargo xtask remote-dev` when testing the current checkout;
the manual launch is mainly useful for a known custom artifact. `remote-dev`
preserves an existing override, so unset `SILICOLAB_DEV_WORKER` when you want it
to select the worker it just built at the default path.

## IDE pre-launch setup

Keep worker compilation as a separate pre-launch task so every IDE run uses the
current worker bytes.

### RustRover

1. Add a Cargo run configuration named **Build development worker** with command
   `xtask build-dev-worker`.
2. Add a Cargo run configuration for SilicoLab with command
   `run --release --features dev-worker`.
3. Set its environment variable
   `SILICOLAB_DEV_WORKER=$ProjectFileDir$/target/x86_64-unknown-linux-musl/release/silicolab-compute`.
4. In **Before launch**, add **Run Another Configuration** and select
   **Build development worker**.

Arguments intended for SilicoLab belong after `--` in the application Cargo
configuration.

### VS Code

Add a build task to `.vscode/tasks.json` (or merge it into the existing file):

```json
{
  "version": "2.0.0",
  "tasks": [
    {
      "label": "silicolab: build development worker",
      "type": "process",
      "command": "cargo",
      "args": ["xtask", "build-dev-worker"],
      "options": { "cwd": "${workspaceFolder}" },
      "problemMatcher": ["$rustc"]
    }
  ]
}
```

Reference it from the Rust launch configuration and select the artifact:

```json
{
  "preLaunchTask": "silicolab: build development worker",
  "env": {
    "SILICOLAB_DEV_WORKER": "${workspaceFolder}/target/x86_64-unknown-linux-musl/release/silicolab-compute"
  }
}
```

The launch configuration itself must build or run the `silicolab` binary with
`--release --features dev-worker`. The exact remaining fields depend on the
installed Rust debugger extension.

## Opt-in SSH integration tests

These tests deploy the current local worker themselves. Do not pre-place a
worker or forge the host's `_worker` cache value.

This section covers the remote suite. The canonical inventory of all
machine-specific engine tests and environment variables is
[Testing external engines](testing-external-engines.md).

First build and validate the artifact:

```text
cargo xtask build-dev-worker
```

Configure a reachable x86_64 Linux host with SilicoLab's dedicated SSH key
authorized for passwordless login, then set the test environment. In
PowerShell:

```powershell
$env:SILICOLAB_TEST_SSH_HOST = 'host-or-ip'
$env:SILICOLAB_TEST_SSH_USER = 'username'
```

In a macOS or Linux shell:

```sh
export SILICOLAB_TEST_SSH_HOST=host-or-ip
export SILICOLAB_TEST_SSH_USER=username
```

Run the direct QM parity test:

```text
cargo test -p compute-core --features dev-worker --test remote_direct -- --ignored --nocapture
```

For a Slurm fixture, also set the host/user variables and run:

```text
cargo test -p compute-core --features dev-worker --test remote_slurm -- --ignored --nocapture
```

The Slurm test deploys the same current-source worker, submits through the shared
bundle launcher, checks CPU, memory, and typed GPU TRES, retrieves a real QM
outcome, and confirms cancellation. It expects the configured partition to be
`debug`, the GPU type to be `rtx4070`, and `sacct` to be unavailable so the
`scontrol` terminal-state fallback is exercised.

The detached frontend tests exercise QM, docking, and GROMACS:

```text
cargo test -p silicolab --features dev-worker --lib -- --ignored remote_qm_submit_then_refresh --nocapture
cargo test -p silicolab --features dev-worker --lib -- --ignored remote_docking_submit_then_refresh --nocapture
cargo test -p silicolab --features dev-worker --lib -- --ignored remote_gromacs_submit_then_refresh --nocapture
```

The GROMACS test requires `gmx` on the host. If a non-interactive SSH shell must
source an environment first, set `SILICOLAB_TEST_GMX_PRELUDE` to that one shell
line. `SILICOLAB_DEV_WORKER` is optional for all of these tests and selects a
non-default local artifact when set.

Three additional frontend tests cover configured and auto-detected GROMACS
launches. To exercise a non-standard executable, set
`SILICOLAB_TEST_GMX_PROGRAM` to an absolute remote path that is neither on
`PATH` nor in the built-in candidate list, then run:

```text
cargo test -p silicolab --features dev-worker --lib -- --ignored remote_gromacs_honors --nocapture
cargo test -p silicolab --features dev-worker --lib -- --ignored verify_confirms_a_remote_gmx --nocapture
cargo test -p silicolab --features dev-worker --lib -- --ignored verify_with_no_program --nocapture
```

The tests are ignored by default because they mutate the configured host's
SilicoLab work directory and require real SSH access. They leave production
release resolution disabled by selecting the local development artifact.

## Troubleshooting

**`rustup` or the musl target is unavailable.** Confirm that `rustup` manages the
active toolchain with `rustup show`, then retry `cargo xtask build-dev-worker`.
The xtask prints the failing `rustup target add` command when automatic
installation fails.

**The build cannot find a linker.** Run the command from this repository so
`.cargo/config.toml` selects `rust-lld` and self-contained linking. Remove local
Cargo or environment overrides that replace the target linker. A system musl
toolchain should not be necessary.

**The artifact is missing or rejected as ELF.** Re-run
`cargo xtask build-dev-worker`. If `SILICOLAB_DEV_WORKER` is set, check that it
points to the Linux cross-target output rather than the host application's
executable. The validator rejects oversized files, truncated files, ELF32,
big-endian ELF, and non-x86-64 machine types before SSH deployment begins.

**Deployment tries to access GitHub Releases.** The application was not started
through the development path. Use `cargo xtask remote-dev`, or ensure a manual
launch enables `dev-worker` and selects a valid local artifact. Development mode
does not fall back to a release download.

**The remote host reports `aarch64` or another architecture.** Select an x86_64
Linux host. Cross-compiling the local worker does not make it portable to a
different remote CPU architecture.

**A cached worker is missing, corrupt, or no longer executable.** No manual cache
editing is needed. SilicoLab verifies a cache hit remotely and redeploys the
selected artifact when the executable is absent or fails its version check. If
redeployment also fails, inspect SSH permissions and free space under the
host's configured work root.

**SSH setup or host-key verification fails.** Follow the end-user
[remote host setup](../docs-site/src/content/docs/getting-started/remote-execution.md#set-up-ssh).
The development path reuses the same hardened SSH/SCP transport and does not
weaken host-key checks.

**A Slurm job stays queued.** Inspect the task monitor's scheduler reason, then
verify the partition, account, QOS, constraint, and requested GPU type. The
capability refresh is a suggestion cache; the scheduler remains authoritative.

**The scheduler test reports that the worker is not visible.** Configure a work
root on a filesystem shared by the login and compute nodes. Deployment through
the login node cannot make a node-local path visible elsewhere.

**`sacct` is unavailable.** This is supported. Active state comes from `squeue`,
and terminal state falls back to `scontrol` for as long as the controller retains
the job record.
