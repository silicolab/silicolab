---
title: Build polymer chains from a repeat unit
description: Grow a linear polystyrene chain from a repeat-unit SMILES in Console, check its residues, and pack several chains into a box.
sidebar:
  order: 8
---

## Goal

Build a 10-unit polystyrene chain from one repeat-unit SMILES, confirm that each unit is recorded
as a residue, and understand what the chain is and is not good for before packing it.

## Build the chain

A repeat unit is a monomer SMILES with two `[*]` attachment points: the first in reading order is
where the previous unit attaches, the second where the next one does. Polyethylene is `[*]CC[*]`;
polystyrene is `[*]CC([*])c1ccccc1`. In **Console** run:

```text
polymer build "[*]CC([*])c1ccccc1" --dp 10 --name "PS 10"
```

The transcript reports `built polymer PS 10`, the new entry's project-dependent number, `162 atoms`,
the radius of gyration, and the end-to-end distance. Both chain ends are capped with hydrogen.

| Check | Result |
| --- | --- |
| Entry name | `PS 10` |
| Structure counts | 162 atoms, 10 residues, 1 chain |
| Formula | `C80H82` |
| Residue names | `MOA` for every unit; `--residue PS` renames them |
| Periodic cell | `none` |

The chain is grown unit by unit. Each new unit tries every staggered rotamer of its backbone bonds
and a full turn of torsions about the new bond, and takes the placement with the fewest contacts,
looking one unit ahead so it does not block the next one. A junction that still has a hard clash is
relaxed with UFF. The result is a self-avoiding, fairly extended chain, not an equilibrium coil;
compress and equilibrate the packed box before measuring anything. The same `--seed` gives the same
chain on every run; a different seed gives a different chain, which is how you make several
distinct chains for a box.

Expect a few seconds for the unit and well under a second per junction without relaxation; the UFF
relaxations dominate otherwise (polystyrene DP 30 takes about 25 s, DP 100 about 90 s in a release
build). If the transcript ends with `N close contacts remain`, the chain has non-bonded atoms closer
than a hard clash limit. Rebuild with another `--seed`, or relax the packed box with `md simulate`
later. `--no-relax` skips the junction relaxation, which is much faster but leaves more contacts.

## Copolymers

List one SMILES per unit, labelled `A`, `B`, … in order, and give the pattern with `--sequence`;
the pattern is cycled to `--dp`:

```text
polymer build "[*]CC[*]" "[*]CC(C)[*]" --dp 20 --sequence AB --name "PE-PP alternating"
```

Units of a copolymer are named `MOA`, `MOB`, … by label.

## What comes next

`polymer build` makes one chain. Pack chains into a simulation box with the **Disordered System**
panel or the `disorder` command, starting well below the target density, then densify with molecular
dynamics. Force-field typing of organic chains is not yet available, so `md build` on a polymer
currently stops at the topology step; a plain `sketch` of a SMILES containing `[*]` is refused
because a wildcard has no 3D meaning.
