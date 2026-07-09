---
title: External tools
description: Configure external programs used by SilicoLab feature modules.
sidebar:
  order: 3
---

SilicoLab can launch without optional external tools. Install these programs
only when you need the features that call them.

## GROMACS

Molecular dynamics simulations require
[GROMACS](https://www.gromacs.org/) to be installed separately.

GPU acceleration is strongly recommended. Running molecular dynamics on CPU
alone is technically possible, but it is usually too slow for non-trivial
systems.

- **Windows:** Install GROMACS inside
  [WSL](https://learn.microsoft.com/en-us/windows/wsl/install) with
  `sudo apt install gromacs` for a quick start. For GPU acceleration, compile
  GROMACS from source with CUDA inside WSL.
- **Linux:** Install the package with `sudo apt install gromacs` for a quick
  start. For production MD, compile from source with CUDA or ROCm support.
- **macOS:** Install with `brew install gromacs`. GPU acceleration is not
  supported on Apple hardware, so MD performance will be limited.

After installation, open SilicoLab and let the engine settings detect the
`gmx` executable before running MD.

## Remote hosts

Heavy calculations can also run on a remote Linux host over SSH. This is useful
when your laptop has no GPU or when the required engine is available only on an
HPC login node or workstation.

See [Remote execution](./remote-execution/) for the SSH setup flow.
