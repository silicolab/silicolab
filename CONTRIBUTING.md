# Contributing

How to build, test, and land changes in SilicoLab. Before you start, read
[ARCHITECTURE.md](ARCHITECTURE.md) â€” it describes the design invariants you are
expected to respect while making changes. When you are adding a new feature,
[docs/adding-a-feature.md](docs/adding-a-feature.md) maps each kind of feature to
the module to mirror and the exact sites to touch.

## Building, running, and testing

Debug builds of the wgpu renderer are slow, so **default to `--release`** for
anything you actually intend to run or look at.

```powershell
cargo build --release
cargo run --release                                 # launch the GUI
cargo run --release -- <script.sls> --name value    # run a script headless
cargo test --release [<name substring>]
cargo test --release -- --ignored wsl_gromacs       # needs WSL + GROMACS
```

(See the README for installing external runtime dependencies such as GROMACS.)

Most tests are inline `#[cfg(test)] mod tests` blocks next to the code they cover.
A top-level `tests/` directory holds the few true integration tests that drive the
compiled binary end-to-end (e.g. `tests/engine_exec.rs`); prefer inline unit tests
and reach for `tests/` only when a test must exercise the built artifact itself.

Machine-specific acceptance tests â€” those that need an external tool installed â€”
are gated with `#[ignore]` so the default `cargo test` run stays hermetic. Run
them explicitly, as shown above.

## Commit discipline

Everything syncs straight to `main`. As a result:

- **Every commit must compile and stand on its own.**
- Each commit should be **one coherent capability**, not a grab-bag of unrelated
  changes.
- Note any known limitations in the commit body.

Two classes of code are gated differently:

- **Generic / infrastructure** (subprocess runner, registry launch model, config
  plumbing): may land once the pre-commit gates below pass.
- **Engine-specific integration** (e.g. GROMACS input generation, grompp/mdrun
  orchestration, output parsing): **must not land until a test exercises the
  real external tool end-to-end.** A `--version` detection test is not
  sufficient â€” integration code that has only been checked against mocks does
  not ship.

When a single change mixes the two, **split the commit**: land the generic layer
now, and hold the integration code until its real-run test exists.

### Pre-commit gates

All of the following must pass before you commit. They mirror the CI merge gate,
which runs with `RUSTFLAGS=-D warnings` across the whole workspace:

1. `cargo fmt --all --check`
2. `cargo clippy --workspace --all-targets --all-features`
3. `cargo test --workspace --all-features`

Set `RUSTFLAGS=-D warnings` so clippy and test fail on warnings exactly as CI does.

## Licensing

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in SilicoLab is submitted under GPL-3.0-or-later and also grants the SilicoLab copyright holders the right to offer that contribution under separate commercial licenses.

By contributing, you confirm that you have the right to make that grant.

## Code conventions
- **Single-responsibility files.** Split into modules rather than letting a file
  accumulate â€” keep source files under 600 *code* lines (soft target ~400), per
  [`.rules`](.rules). This is a modular Cargo workspace; lean on the module system.