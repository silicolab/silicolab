---
title: Installation
description: Install SilicoLab from a prebuilt executable or build it from source.
sidebar:
  order: 1
---

## Download and install

Download packages from [GitHub Releases](https://github.com/silicolab/silicolab/releases).
On macOS 13 or later, choose `macos-aarch64.dmg` for Apple Silicon or
`macos-x86_64.dmg` for Intel. Open the DMG and drag the large SilicoLab icon on the
left onto Applications on the right. Eject the image, then launch SilicoLab from
Applications in Finder; no terminal command is needed.

The current DMGs contain **ad-hoc-signed, unnotarized** apps. Local signature
integrity and local launching do not establish Internet-download acceptance.
Gatekeeper may block a downloaded copy; follow its actual message and Apple's
[app safety guidance](https://support.apple.com/en-us/102445). Do not disable
Gatekeeper or ignore malware/damaged-app warnings.

To update a DMG installation, quit SilicoLab and replace the complete app from the
new DMG. Projects and settings are kept. Windows/Linux portable archives and the
existing macOS archive layout remain available with their original update behavior.

Projects remain directories of SQLite databases; open them using **Open Project**
in the app. `.sls` scripts keep their explicit CLI/in-app execution path and are
not registered for automatic Finder execution. This package does not include
external compute engines or change remote-worker deployment.

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

See [External tools](../external-tools/) for setup notes, including GPU
acceleration. See [Remote execution](../remote-execution/) to run heavy jobs on
a remote Linux host over SSH.
