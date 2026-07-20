---
title: Work with periodic cells and supercells
description: Build periodic graphene, expand it in place, and distinguish cells, coordinate representations, wrapping, and replication.
sidebar:
  order: 4
---

## Goal

Build a `4 x 4 x 1` graphene sheet in an empty project, then expand the active entry in place with
explicit `2 x 2 x 1` repeats. The expected change is from `C32`, 32 atoms, and 48 stored bonds to
`C128`, 128 atoms, and 192 stored bonds.

## Build the starting cell

Open **Launch**, expand **Structure Builder**, and choose **Nanosheet Builder**. Enter every value explicitly:

| Setting | Value |
| --- | --- |
| Sheet | `Graphene` |
| Sublattice A / B | `C` / `C` |
| Lattice a | `2.46 Å` |
| Buckling | `0 Å` |
| Interlayer spacing (A) | `12 Å` |
| Supercell | `4 x 4 x 1` |

For zero-buckling graphene, **Interlayer spacing (A)** is the separation between periodic images
along `c`; it therefore produces the stored `c` length of `12.000 Å`.

Choose **Build**. A transient success notice appears, **Activity** records **Nanosheet Builder** as
`Completed`, and a new entry becomes active. Continue only when Details shows all of these checks
together:

| Check | Expected value |
| --- | --- |
| Formula / atoms / stored bonds | `C32` / 32 / 48 |
| Stored graph | 1 connected component |
| Cell lengths | `9.840 x 9.840 x 12.000 Å` |
| Cell angles | `90.000 / 90.000 / 60.000°` |

The transient notice and `Completed` status show that the build operation ended. The stored entry and
its Details values are the persistent result checks.

## Expand the active entry

Keep the checked `C32` entry active. In **Launch**, expand **Structure Editing**, choose
**Expand Supercell**, and set the three repeat factors to `2`, `2`, and `1`. Do not rely on values
already present in the panel.

Choose **Expand**. This operation modifies the active entry in place. A transient notice reports
`Expanded to 2x2x1 supercell (128 atoms, 192 bonds)`, and **Activity** records
**Supercell Expansion** as `Completed`.

| Check | Before expansion | After expansion |
| --- | ---: | ---: |
| Formula | `C32` | `C128` |
| Atoms | 32 | 128 |
| Stored bonds | 48 | 192 |
| Stored graph | 1 connected component | 1 connected component |
| Cell lengths | `9.840 x 9.840 x 12.000 Å` | `19.680 x 19.680 x 12.000 Å` |
| Cell angles | `90.000 / 90.000 / 60.000°` | `90.000 / 90.000 / 60.000°` |

The repeat product is `2 x 2 x 1 = 4`, so the atom and stored-bond counts both increase by a factor
of 4. The `a` and `b` lengths double, while `c` and all cell angles remain unchanged.

## Cell, coordinates, and display

SilicoLab stores the cell separately from atom positions. Cell lengths use Å and cell angles use
degrees. Cartesian coordinates express positions in Å. Fractional coordinates use the lattice
vectors as their basis, so `(1, 0, 0)` is one complete `a` vector.

For a periodic structure, **Edit Structure...** initially presents fractional coordinates and can
switch to Cartesian. Switching representation does not move atoms. Fractional coordinates also do
not have to lie in `[0, 1)` until a wrap operation maps each component into that interval.

The six values in Details are the stored cell. **Style > Scene > Unit cell** only shows or hides the
cell wireframe in the viewport. Hiding the wireframe does not remove the cell, and showing it does
not prove that the stored values are correct.

## Wrapping is not expansion

| Operation | Result |
| --- | --- |
| **Edit > Edit Structure... > Wrap into cell > Apply** | Wraps atoms in the edit draft and recomputes bonds from the wrapped coordinates when the edit is applied |
| **Launch > Structure Editing > Wrap Into Cell** | Wraps the active periodic entry in place while preserving its stored bonds |
| **Launch > Structure Editing > Expand Supercell** | Copies atoms and the stored bond graph, enlarges the cell by the repeat factors, and modifies the active entry in place |

Both wrap paths process atoms individually rather than translating each complete molecule as one
unit. A molecule crossing a periodic boundary may therefore appear split across opposite sides of
the first cell. Wrapping does not increase atom counts or cell lengths; expansion replicates the
structure and enlarges the cell.

## Recover to a known state

| Symptom | Recovery | Check before continuing |
| --- | --- | --- |
| The graphene settings or starting counts are wrong | Cancel the panel and rebuild in an empty project with every fixed value above | `C32`, 32 atoms, 48 bonds, 1 component, and the original cell agree |
| The wrong repeats were applied | If expansion was the last edit, use Undo immediately; otherwise rebuild the starting sheet | The active entry returns to the complete `C32` checkpoint |
| Expansion says the structure has no periodic cell | Stop, activate the checked graphene entry, and inspect Details | All 6 cell parameters are present; a visible wireframe alone is insufficient |
| Wrapping split a molecule or changed bonds unexpectedly | Use Undo immediately; rebuild the fixed starting point if the prior state is uncertain | Formula, atoms, bonds, components, and cell all match the pre-operation state |
| Only the wireframe is missing | Re-enable **Unit cell** in Style | Details remains unchanged |

Stop the workflow whenever the formula, counts, connected components, or any cell parameter differs
from the matching checkpoint. Do not continue from a structure whose pre-operation state is uncertain.

## Scientific limits

Building and expansion do not relax or optimize the geometry. Expansion copies the recorded atoms,
cell, and bond graph; it does not infer missing periodic-image bonds, reconstruct crystallographic
symmetry, resolve partial occupancy, normalize a source structure, or test all overlaps.

The wireframe, replication arithmetic, formula, and `Completed` state are structural and software
checks only. They do not establish energetic stability, a valid defect model, simulation readiness,
or experimental meaning.

## Related pages

- [Build disordered starting geometries](../disordered-systems/)
- [Build nanosheets and annotate building blocks](../nanosheets-and-building-blocks/)
- [Edit and export structures](../../projects-structures/edit-and-export/)
