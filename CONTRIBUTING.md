# Contributing

How to build, test, and land changes in SilicoLab. Before you start, read
[ARCHITECTURE.md](ARCHITECTURE.md) — it describes the design invariants you are
expected to respect while making changes.

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

Tests are inline `#[cfg(test)] mod tests` blocks next to the code they cover —
there is **no `tests/` directory**. Add tests alongside the code, not in a
separate tree.

Machine-specific acceptance tests — those that need an external tool installed —
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
  sufficient — integration code that has only been checked against mocks does
  not ship.

When a single change mixes the two, **split the commit**: land the generic layer
now, and hold the integration code until its real-run test exists.

### Pre-commit gates

All of the following must pass before you commit:

1. `cargo fmt --check`
2. `cargo clippy --all-targets --release -- -D warnings`
3. `cargo test --release`

## Code conventions

- **Single-responsibility files.** Split into modules rather than letting a file
  accumulate — target under 500 lines, with real discomfort past ~800. This is a
  modular single-crate project; lean on the module system.