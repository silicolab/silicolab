---
title: External tools
description: Configure external programs used by SilicoLab feature modules.
sidebar:
  order: 5
---

SilicoLab can launch without optional external tools. Install these programs
only when you need the features that call them.

## ORCA

The built-in Hartree engine is the default for quantum chemistry. ORCA is
an optional alternative for molecular single-point energies, geometry
optimizations, and vibrational frequencies. Transition-state and periodic QM
calculations currently use Hartree.

Install ORCA separately, then open **Settings > Compute > Compute targets**. In
the ORCA row for the relevant target, enter the executable path under
**Program** and select **Verify**. SilicoLab deliberately does not search for
ORCA.

For ORCA inside WSL on Windows, set **Command prefix** to `wsl.exe -e` and set
**Program** to the executable's Linux path, such as `/opt/orca/orca`. For a
native installation, leave the command prefix empty. Select ORCA explicitly in
the QM task panel or use `qm energy --engine orca` in a script.

ORCA starts with one CPU core. Requesting more cores enables ORCA's `%pal`
parallel mode and requires `mpirun` to be available in the target environment.

Remote hosts have their own ORCA program setting. Enter a path valid on that
host; a local path is never copied to a remote target.

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

After installation, open **Settings > Compute > Compute targets** and expand the
target you will run on. In its GROMACS row, enter the executable under
**Program**, or leave **Program** empty so **Verify** searches that target, then
select **Verify** before running MD.

## Remote hosts

Heavy calculations can also run on a remote Linux host over SSH. This is useful
when your laptop has no GPU or when the required engine is available only on an
HPC login node or workstation.

See [Remote execution](../remote-execution/) for the SSH setup flow.
