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

## License

SilicoLab is available under either:

- [GPL-3.0-or-later](LICENSES/GPL-3.0-or-later.txt), or
- a separate commercial license granted in writing by the SilicoLab copyright holders.

If you do not have a signed commercial license agreement, your rights are under GPL-3.0-or-later. The repository records this dual-license structure with REUSE/SPDX metadata in [REUSE.toml](REUSE.toml): GPL-3.0-or-later OR LicenseRef-SilicoLab-Commercial.

Third-party components remain under their own licenses; see [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md).

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in this work by you is submitted under GPL-3.0-or-later and also grants the SilicoLab copyright holders the right to offer that contribution under separate commercial licenses.
