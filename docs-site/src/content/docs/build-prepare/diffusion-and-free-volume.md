---
title: Diffusion and free volume in a dense phase
description: Measure the probe-accessible free volume of a packed box and the self-diffusion coefficient of sorbate molecules from an MD trajectory in Console.
sidebar:
  order: 9
---

## Goal

Take a packed or simulated box (a polymer membrane with sorbed guests, a liquid, a glass) and get
the two numbers a transport argument rests on: how much free volume a probe of a given size can
reach, and how fast each sorbate species moves.

## Free volume

`membrane free-volume` lays a grid over the periodic cell and counts the points that no atom's van
der Waals sphere, grown by the probe radius, covers. The active entry must have a cell: a box from
`disorder`, `md build`, or an MD run.

```text
pack --of active --count 20 --box 18,18,18
membrane free-volume --probe 1.0
```

The reply reads `free volume 30.7% of 5832 Å³ (1792 Å³) with a 1.0 Å probe on a 0.50 Å grid;
density 0.445 g/cm³`. Options:

| Option | Meaning |
| --- | --- |
| `--probe <Å>` | Probe radius (default 1.0 Å). `0` gives the plain unoccupied fraction; 1.3 Å mimics the positronium probe of PALS experiments. |
| `--grid <Å>` | Grid spacing (default 0.5 Å). Halving it multiplies the cost by about eight for about one percentage point of precision. |
| `--trajectory <path> --frames N` | Average over the last `N` frames of a trajectory instead of the entry's own coordinates, using each frame's box. With `--frames` alone, the entry's recorded MD trajectory is used. |

The fraction is geometric: it depends on the Bondi radii and the probe, not on a force field, and
it is only as good as the packing. A box straight out of `disorder` is far too dilute; the number is
meaningful after `md simulate` has compressed and equilibrated the system.

## Diffusion

`membrane diffusion` tracks the centre of mass of each selected molecule through a trajectory,
computes the mean-squared displacement (MSD) over every time origin, and fits the Einstein relation
`MSD = 6 D t` over the middle of the lag range.

```text
membrane diffusion --select BEN --out msd.csv
```

Without a path the entry's recorded MD trajectory is read; pass a `.xtc`, `.trj`, or `.xyz` file
to analyse another run of the same topology. The reply reads `D = 7.9e-6 cm²/s for 20 molecules
(`BEN`) over 60 frames of md.xtc; MSD exponent 1.12 on 14–48 ps`.

| Option | Meaning |
| --- | --- |
| `--select` | `all`, an element symbol, or a residue name (`BEN` for the benzene copies `disorder` packed). Bonded atoms within the selection form one molecule. |
| `--max-lag <ps>` | Longest lag to evaluate (default: half the trajectory). |
| `--fit from,to` | Fraction of the lag range to fit (default `0.2,0.8`). |
| `--out <csv>` | Write the MSD curve (`lag_ps,msd_angstrom2`) for plotting. |

Coordinates are unwrapped through each frame's box (or the entry's cell for an XYZ trajectory), so
GROMACS' wrapped `.xtc` output needs no preprocessing. Watch the exponent: the log–log slope of the
MSD over the fit window is 1 for Fickian diffusion. A value well below 1 means the sorbate is still
caged in its hole and the run is too short; the command says so. An XYZ trajectory needs a
`t=` time (ps) in each frame's comment line; without one, frames are taken as 0.005 ps apart and
the reply warns that D rests on that spacing. Frame times must be evenly spaced and increasing: a
trajectory joined from restarts with a repeated frame, or one with a gap, is refused rather than
fitted on a wrong time axis.

A permeability-style selectivity follows from two runs: the ratio of the diffusion coefficients of
two sorbates in the same membrane times the ratio of their sorption. The sorption half is a
separate step.
