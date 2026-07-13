# Testing external engines

Machine-specific acceptance tests are ignored by default. Run them when a
change touches engine discovery, launch configuration, input generation,
execution, or output parsing. Unit tests and mocks do not replace these tests
for engine-specific integration code.

Run ordinary hermetic tests first:

```text
cargo test --release --workspace --all-features
```

## Local GROMACS through WSL

These tests require Windows, WSL, and a working `gmx` inside WSL. They share
working paths, so run them serially:

```text
cargo test --release -p compute-core -- --ignored wsl_gromacs --test-threads=1 --nocapture
```

To test a GROMACS executable outside the normal candidate paths, set
`SILICOLAB_TEST_WSL_GMX` to its absolute Linux path before running the same
command.

The frontend command-path acceptance test is separate:

```text
cargo test --release -p silicolab --lib -- --ignored agent_md_simulate --nocapture
```

## Local ORCA

Set `SILICOLAB_TEST_ORCA_PROGRAM` to the ORCA executable. If it must be launched
through a wrapper, set `SILICOLAB_TEST_ORCA_PREFIX` to the prefix tokens. Then
run both the core adapter and compiled-binary integration tests:

```text
cargo test --release -p compute-core --lib -- --ignored configured_orca --nocapture
cargo test --release -p silicolab --test engine_exec -- --ignored exec_subcommand_runs_configured_orca --nocapture
```

## Remote direct and Slurm execution

Remote tests mutate the configured host's SilicoLab work directory. Use a
disposable fixture or dedicated test directory, and build the current worker
before running them:

```text
cargo xtask build-dev-worker
```

Required environment:

| Variable | Meaning |
|---|---|
| `SILICOLAB_TEST_SSH_HOST` | Reachable x86_64 Linux SSH host. |
| `SILICOLAB_TEST_SSH_USER` | SSH user; defaults to `root` in the tests. |
| `SILICOLAB_DEV_WORKER` | Optional non-default local worker artifact. |
| `SILICOLAB_TEST_GMX_PRELUDE` | Optional shell line run before remote `gmx`. |
| `SILICOLAB_TEST_GMX_PROGRAM` | Optional non-standard absolute remote `gmx` path. |

The complete commands and Slurm fixture assumptions live in
[Developing remote execution](developing-remote-execution.md#opt-in-ssh-integration-tests).

## GPU-dependent renderer tests

GPU adapter tests are ignored on headless machines. Run them on a machine with
a supported graphics adapter and driver:

```text
cargo test --release -p silicolab --lib -- --ignored gpu_ --nocapture
```

Treat a test that prints `skip:` because a required environment variable is
missing as not executed, not as acceptance evidence.
