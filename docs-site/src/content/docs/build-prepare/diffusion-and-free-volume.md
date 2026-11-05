---
title: Diffusion and free volume in a dense phase
description: Measure the probe-accessible free volume of a packed box, the self-diffusion coefficient of sorbate molecules, their sorption by Widom insertion, and radial distribution functions in Console.
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
two sorbates in the same membrane times the ratio of their sorption. `membrane sorption` supplies
the sorption half.

## Sorption

`membrane sorption` performs Widom test-particle insertion: a copy of the probe molecule is dropped
at random positions and orientations into the host box, its interaction energy with the host is
evaluated, and the Boltzmann average `⟨exp(−ΔU/kT)⟩` gives the excess chemical potential
`μ_ex = −kT ln⟨…⟩` and the infinite-dilution solubility coefficient `S = exp(−μ_ex/kT) / (R T)`
in cm³(STP)/(cm³·cmHg).

```text
forcefield type benzene --name benzene
membrane sorption benzene --frames 20
```

The host is the active entry's typed MD system (`md build` from library-typed molecules) and the
probe is another entry whose molecule is typed in the force-field library, so both carry
Lennard-Jones parameters and partial charges. The reply reads `benzene: μ_ex = -14.2 kJ/mol,
⟨exp(−ΔU/kT)⟩ = 2.96e2 ± 4.1e1, S = 3.550e0 cm³(STP)/(cm³·cmHg) at 300 K from 400000
insertions over the last 20 frames of md.xtc (22.4% favourable)`.

| Option | Meaning |
| --- | --- |
| `--entry` | Host entry (default `active`). |
| `--temperature <K>` | Insertion temperature (default 300 K). |
| `--insertions N` | Trial insertions per frame (default 20000). The quoted error is the standard error of the Boltzmann mean. |
| `--cutoff <Å>` | Pair cutoff (default 10 Å); the box must be at least twice as wide. Lennard-Jones beyond it is added as the analytic tail of a uniform host; Coulomb is shifted-force so the sum does not depend on which atoms of a molecule straddle the sphere. |
| `--seed N` | Random seed; the same seed reproduces the same insertions. |
| `--trajectory <path> --frames N` | Average over the last `N` frames instead of the entry's own coordinates. With `--frames` alone, the entry's recorded MD trajectory is used. |

Insertion is cheap but blind: in a dense glassy polymer most trials land inside an atom and only
the few that find a cavity count, so take many frames and expect the error bar to shrink slowly.
A probe larger than the free-volume elements cannot be inserted at all and reports a vanishing
Boltzmann mean; the number is then a statement about the method, not the material. Solubility
selectivity is the ratio of `S` for two probes in the same frames.

## Radial distribution

`membrane rdf` histograms the minimum-image distances between two selections and normalises them
to the ideal-gas expectation, giving `g(r)` and the running coordination number.

```text
membrane rdf BEN C --frames 20 --out rdf.csv
membrane rdf O
```

With one selection the pairs are within it. The reply names the first peak; `--out` writes
`r_angstrom,g,coordination`.

| Option | Meaning |
| --- | --- |
| `--entry` | Host entry (default `active`). |
| `--rmax <Å>` | Longest separation tabulated (default 12 Å); must not exceed half the box. |
| `--bin <Å>` | Bin width (default 0.1 Å). |
| `--trajectory <path> --frames N` | Average over trailing frames as above. |
