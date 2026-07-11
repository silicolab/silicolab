<p align="left">
  <img src="assets/icon/window-256.png" alt="" height="42" align="middle">
  &nbsp;&nbsp;
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="branding/wordmark-dark.svg">
    <img src="branding/wordmark-light.svg" alt="SilicoLab" height="44" align="middle">
  </picture>
</p>

<p align="left"><em>Computational environment for chemistry, biology &amp; materials research.</em></p>

<p align="left">
  <a href="https://silicolab.github.io/silicolab/"><img alt="Documentation" src="https://img.shields.io/badge/docs-silicolab.github.io-2563eb"></a>
  <a href="https://github.com/silicolab/silicolab/actions/workflows/ci.yml"><img alt="CI" src="https://github.com/silicolab/silicolab/actions/workflows/ci.yml/badge.svg?branch=main"></a>
  <a href="https://github.com/silicolab/silicolab/actions/workflows/docs.yml"><img alt="Docs" src="https://github.com/silicolab/silicolab/actions/workflows/docs.yml/badge.svg?branch=main"></a>
  <a href="https://github.com/silicolab/silicolab/releases"><img alt="Release" src="https://img.shields.io/github/v/release/silicolab/silicolab?include_prereleases&sort=semver"></a>
  <a href="LICENSE"><img alt="License" src="https://img.shields.io/badge/license-GPL--3.0--or--later%20OR%20commercial-blue"></a>
</p>

**Read the [user manual](https://silicolab.github.io/silicolab/) or download a
prebuilt executable from [GitHub Releases](https://github.com/silicolab/silicolab/releases).**

![SilicoLab screenshot](docs/images/main-window.png)

## Features

- Interactive 3D visualization and editing of molecular and crystal structures
- 2D molecule sketcher - draw a molecule (atoms, bonds, ring/fragment templates,
  charges) on a canvas and build it into a real 3D structure; also import/export
  SMILES, with a scriptable `sketch <SMILES>` command in the console and CLI
- Force-field geometry optimization
- Quantum chemistry calculations
- Guided molecular dynamics setup and execution (powered by GROMACS)
- Reticular structure builder - assemble COFs and MOFs from building blocks
- One scripting language for everything: the same scripts run in the GUI
  console and headless on the CLI, making workflows easy to automate and
  agent-friendly

## Installation

### Prebuilt executables

Download prebuilt executables from
[GitHub Releases](https://github.com/silicolab/silicolab/releases).

### Build from source

Install the [Rust toolchain](https://rustup.rs), then build the release
executable:

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

The same scripts also run interactively in the GUI console. The full user
manual in English and Chinese lives at <https://silicolab.github.io/silicolab/>.

## External tools

SilicoLab can run without optional external tools until you use features that
need them. Molecular dynamics requires GROMACS. Quantum chemistry uses the
built-in Hartree engine by default, with ORCA available as an optional external
engine when the user configures its executable path. See the manual for setup details:

- [External tools](https://silicolab.github.io/silicolab/getting-started/external-tools/)
- [Remote execution over SSH](https://silicolab.github.io/silicolab/getting-started/remote-execution/)

## Development

- [Contributing guide](CONTRIBUTING.md)
- [Architecture notes](ARCHITECTURE.md)
- [Feature wiring guide](docs/adding-a-feature.md)

## License

SilicoLab is available under either:

- [GPL-3.0-or-later](LICENSES/GPL-3.0-or-later.txt), or
- a separate commercial license granted in writing by the SilicoLab copyright holders.

If you do not have a signed commercial license agreement, your rights are under GPL-3.0-or-later. The repository records this dual-license structure with REUSE/SPDX metadata in [REUSE.toml](REUSE.toml): GPL-3.0-or-later OR LicenseRef-SilicoLab-Commercial.

Third-party components remain under their own licenses; see [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md).

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in this work by you is submitted under GPL-3.0-or-later and also grants the SilicoLab copyright holders the right to offer that contribution under separate commercial licenses.
