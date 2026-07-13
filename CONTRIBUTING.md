# Contributing

How to build, test, and land changes in SilicoLab. Before you start, read
[ARCHITECTURE.md](ARCHITECTURE.md) - it describes the design invariants you are
expected to respect while making changes. When you are adding a new feature,
[docs/adding-a-feature.md](docs/adding-a-feature.md) maps each kind of feature to
the module to mirror and the exact sites to touch.

## Building, running, and testing

Debug builds of the wgpu renderer are slow, so **default to `--release`** for
anything you actually intend to run or look at.

Install the repository toolchain with `rustup`. On Debian or Ubuntu, the GUI
also needs the same system libraries CI installs:

```sh
sudo apt-get update
sudo apt-get install -y libgtk-3-dev libxkbcommon-dev libwayland-dev \
  libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev
```

Windows and macOS need no additional build packages. Common commands are:

```powershell
cargo build --release
cargo run --release                                 # launch the GUI
cargo run --release -- <script.sls> --name value    # run a script headless
cargo test --release --workspace --all-features [<name substring>]
cargo test --release -p compute-core -- --ignored wsl_gromacs --test-threads=1
```

The last command needs WSL and GROMACS. See
[Testing external engines](docs/testing-external-engines.md) for GROMACS, ORCA,
GPU, and SSH acceptance-test prerequisites and environment variables.

Most tests are inline `#[cfg(test)] mod tests` blocks next to the code they cover.
A top-level `tests/` directory holds the few true integration tests that drive the
compiled binary end-to-end (e.g. `tests/engine_exec.rs`); prefer inline unit tests
and reach for `tests/` only when a test must exercise the built artifact itself.

Machine-specific acceptance tests - those that need an external tool installed -
are gated with `#[ignore]` so the default `cargo test` run stays hermetic. Run
them explicitly using the commands in the external-engine testing guide.

## Remote execution development

When a change affects the remote worker, a remote-capable engine, or the wire
contract, run the current checkout through the development worker path:

```powershell
cargo xtask remote-dev
```

For an IDE pre-launch task, use `cargo xtask build-dev-worker`. Ordinary builds
remain pinned to published, checksum-verified workers and cannot test
unpublished source changes. See
[Developing remote execution](docs/developing-remote-execution.md) for platform
prerequisites, artifact overrides, IDE setup, deployment identities, and the
opt-in SSH integration tests.

## Commit discipline

All changes land through a pull request targeting `main`; there is no long-lived
development branch. As a result:

- **Every commit must compile and stand on its own.**
- Each commit should be **one coherent capability**, not a grab-bag of unrelated
  changes.
- Note any known limitations in the commit body.
- Use a [Conventional Commits](https://www.conventionalcommits.org/) subject,
  matching the existing history.

Two classes of code are gated differently:

- **Generic / infrastructure** (subprocess runner, registry launch model, config
  plumbing): may land once the pre-commit gates below pass.
- **Engine-specific integration** (e.g. GROMACS input generation, grompp/mdrun
  orchestration, output parsing): **must not land until a test exercises the
  real external tool end-to-end.** A `--version` detection test is not
  sufficient - integration code that has only been checked against mocks does
  not ship.

When a single change mixes the two, **split the commit**: land the generic layer
now, and hold the integration code until its real-run test exists.

### Pre-commit gates

Run the local PR check before you commit:

```powershell
cargo pr-check
```

The command mirrors CI's local Rust gates, including warning-as-error builds. It
does not replace CI's cross-OS matrix or the ignored machine-specific acceptance
tests.

Documentation-only changes must also pass this command. It validates repository
documentation and builds public Rust API documentation with warnings denied.

## Licensing

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in SilicoLab is submitted under GPL-3.0-or-later and also grants the SilicoLab copyright holders the right to offer that contribution under separate commercial licenses.

By contributing, you confirm that you have the right to make that grant.

## Code conventions

- **Single-responsibility files.** Split into modules rather than letting a file
  accumulate; see [`.rules`](.rules) for the current size budget. This is a
  modular Cargo workspace; lean on the module system.
