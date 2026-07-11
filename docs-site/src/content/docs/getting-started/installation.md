---
title: Installation
description: Install SilicoLab from a prebuilt executable or build it from source.
sidebar:
  order: 1
---

## Prebuilt executables

Download the executable for your platform from
[GitHub Releases](https://github.com/silicolab/silicolab/releases) and run it —
no installer required.

## Build from source

Install the [Rust toolchain](https://rustup.rs), then build the release
executable:

```sh
cargo build --release
```

The binary is written to `target/release/` (`silicolab` on Linux/macOS,
`silicolab.exe` on Windows).

## Optional external tools

Some features call external programs at run time. You can install them later —
SilicoLab works without them until you use the corresponding feature.

- **GROMACS** — required for molecular dynamics simulations.

ORCA is an optional external engine for molecular single-point energies,
geometry optimizations, and vibrational frequencies. Quantum chemistry uses
the built-in Hartree engine by default; ORCA is never required or selected
automatically.

See [External tools](./external-tools/) for setup notes, including GPU
acceleration. See [Remote execution](./remote-execution/) to run heavy jobs on
a remote Linux host over SSH.
