---
title: Build reticular structures
description: Assemble the built-in HCB BENZ framework and verify its composition, cell, and layer connectivity.
sidebar:
  order: 6
---

## Goal

Build the default `2 x 2 x 2` **HCB (T3 core + T3 core)** stack from the built-in BENZ components.
The current default result must match all of these values:

| Check | Expected value |
| --- | --- |
| Formula / atoms / stored bonds | `C96H48` / 144 / 168 |
| Cell lengths | Approximately `15.637 x 15.637 x 7.200 Å` |
| Cell angles | `90 / 90 / 60°` |
| Stored graph | 2 connected components, one per layer |
| Interlayer covalent bonds | 0 |

If any value differs, stop and return to the fixed settings below. A two-layer view or a visible
hexagonal cell does not replace these checks.

## Set the fixed draft

Start in a clean writable project. Open **Launch**, expand **Structure Builder**, and choose
**Reticular Builder**. Enter or confirm every value explicitly:

| Setting | Value |
| --- | --- |
| Name | `Structure` |
| Network | `HCB (T3 core + T3 core)` |
| Core A / Core B | `BENZ / BENZ` |
| Linkers | `0 in chain`; add no linker |
| Orientational modulation | Off for Core A and Core B |
| Functionalization | Off |
| Preview supercell | `2 x 2 x 2` |
| Layer spacing | `3.6 Å` |
| Custom building blocks | `0 custom blocks loaded`; do not import one for this workflow |

Both BENZ components are T3 cores with three connection sites. This workflow needs no input file.

## Preview and build

1. Choose **Preview** and inspect the draft. Preview changes the viewport but is not an accepted
   result entry.
2. Recheck every fixed setting. If the draft was changed, correct it and preview again.
3. Choose **Build** only when the settings agree. SilicoLab creates and activates a new entry named
   `Structure`; **Activity** records **Reticular Structure Builder** as `Completed`.
4. Inspect the active entry in Details and the structure summary, then inspect its stored bond graph.
   Continue only if formula, counts, cell, components, and interlayer connectivity match the complete
   checkpoint above.

The `c` length is `2 x 3.6 Å = 7.200 Å`, corresponding to two layer repeats. Two connected
components are therefore expected here and do not indicate a failed build.

## Bond, charge, and file meaning

- Bond types already present inside each BENZ component are retained. New connections between
  different cores are single bonds.
- Newly connected atom pairs are placed at a UFF equilibrium bond length. This is an initial
  coordinate-placement rule, not UFF energy minimization and not a complete force-field model.
- Every output atom receives a stored charge of `0.0`; component charges are not carried into the
  result. This value is not a validated partial charge or oxidation state.
- The two layers are separated by `3.6 Å` and have no interlayer covalent bonds. The stored graph
  does not represent van der Waals or electrostatic interactions, so stacking alone does not show
  interlayer binding or stability.
- The entry receives the suggested relative path `Structure.cif`, but Build does not write a CIF
  automatically. Use the export workflow and choose a destination when a file is required.

## Recover to a known state

| Symptom | Recovery | Check before continuing |
| --- | --- | --- |
| BENZ/BENZ is not selected, or a linker, orientation option, or functionalization is enabled | Restore every fixed value and choose **Preview** again | HCB T3/T3, BENZ/BENZ, no linker, both orientation options off, and functionalization off |
| The preview state is uncertain | Choose **Cancel** before Build, reopen the builder, and restore the fixed draft | No result entry has been accepted |
| Build was used with incorrect settings | Start again in a clean writable project; do not continue from the incorrect result | The new entry matches the complete quantitative checkpoint |
| Any formula, count, cell value, or connectivity value differs | Stop; recheck every setting and rebuild | Only the matching `2 x 2 x 2` result belongs to this workflow |
| `Structure.cif` is not present on disk | Keep the checked result active and export it explicitly | Confirm the chosen format and destination |

Do not continue into editing, export, or calculation while any structural checkpoint is uncertain.

## Scientific limits

This workflow covers the built-in HCB T3/T3 network only. Custom SLF components are outside its
scope; do not use untrusted or unverified component metadata.

Building assembles templates and initial coordinates. It does not restore component charges, create
a validated charge model or complete force-field parameters, minimize or equilibrate the structure,
or establish pore accessibility, mechanical or thermodynamic stability, synthetic accessibility, or
material identity. Validate topology, valence, charge, periodic connectivity, geometry,
parameterization, and any required relaxation before downstream calculation or interpretation.

## Related pages

- [Build periodic nanosheets and annotate building blocks](../nanosheets-and-building-blocks/)
- [Work with periodic cells and supercells](../periodic-cells-and-supercells/)
- [Edit and export structures](../../projects-structures/edit-and-export/)
