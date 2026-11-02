---
title: MD systems of packed organic molecules
description: Type each molecule with acpype, pack chains and guests into a box, and let md build assemble the GAFF2 topology for GROMACS.
sidebar:
  order: 10
---

## Goal

Run GROMACS on a bulk organic system that has no biomolecular residue templates: polymer chains,
a fullerene, solvent or sorbate molecules, in any mix. The force-field work happens once per
distinct molecule; the box can then be rebuilt and repacked freely.

## 1. Type each distinct molecule

`forcefield type` sends one molecule through acpype (see
[External tools](../getting-started/external-tools/)) and stores the typed topology in the
force-field library under a name. Type every kind of molecule the box will contain, with the
exact atoms that will be packed: a capped oligomer rather than the bare repeat unit, the guest as
sketched.

```text
sketch "CC(c1ccccc1)CC(c1ccccc1)CC(c1ccccc1)C" --name ps3
forcefield type ps3 --name ps3
sketch c1ccccc1 --name benzene
forcefield type benzene --name benzene
```

AM1-BCC charges take seconds to minutes for a few dozen atoms and grow steeply with size, so type
short capped oligomers rather than whole chains. `forcefield list` shows what the library holds.

## 2. Pack the box

`disorder` (alias `pack`) places copies of several entries into one cell and records each copy as
a residue; the residue name is the first three letters of the entry name, uppercased:

```text
pack --of ps3 --count 8 --of benzene --count 20 --box 32,32,32 --seed 1
```

Use `--density` instead of `--count` to size the chain count by mass density. A low starting
density (0.1–0.3 g/cm³) packs quickly; the compression stages of the run bring it to the real one.

## 3. Build and run

```text
md build
md simulate --time 500ps --temperature 300
```

`md build` finds the molecules of the active structure (the connected components of its bonds),
matches each to the library entry with the same atoms in the same order and the same bonds between
them (an isomer packed in the same element order is refused), and captures a GAFF2
topology: the typed atoms with their AM1-BCC charges, the bonded terms with acpype's explicit
parameters, and the molecule counts in coordinate order. The reply lists the composition, for
example `… atoms as 8× ps3, 20× benzene`.

When two library entries have the same atoms (two conformers of one chain typed separately, or a
molecule typed with both `gaff` and `gaff2`), name the one to use per residue:

```text
md build --typed PS3=ps3,BEN=benzene
```

The run then goes through the usual `md simulate` / `md run` path, locally or on a remote host.

## What to expect

- Every molecule in the box must have a library match; a molecule with none stops the build and
  names its position and formula. Monatomic ions are not covered yet.
- The run uses the AMBER nonbonded treatment GAFF2 was fitted with: PME electrostatics,
  potential-shift Lennard-Jones and a dispersion correction.
- The topology is only as good as the typing: an oligomer typed once is reused for every copy, so
  its end groups carry the capped charges.
