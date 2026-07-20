---
title: Build disordered starting geometries
description: Pack a deterministic periodic argon example and evaluate complete, partial, stopped, and timed-out results.
sidebar:
  order: 5
---

## Goal

Configure a deterministic request that packs 8 rigid copies of a monatomic argon entry into a
`16 x 16 x 16 Å` cube with spacing `2.0 Å` and seed `3`. Check the stored output structure and cell,
and understand which packing fields the current GUI does and does not retain.

## Fixed sample and request

Download [`BP-ARGON-01`](../../samples/argon.xyz). The fixture contains 1 Ar atom, 0 bonds, and no
cell. It does not include force-field parameters, a thermodynamic state, or experimental provenance.

Import `argon.xyz` into a new project and explicitly activate it. Open **Launch**, expand
**Molecular Dynamics**, and choose **Disordered System**.

| Setting | Value |
| --- | --- |
| Result name | `Periodic argon` |
| Specify amount by | `Copies` |
| Molecule / amount | `BP-ARGON-01` / `8 copies` |
| Region / size | `Box` / `16 x 16 x 16 Å` |
| Result cell | Enable **Use the region as the result's simulation cell** |
| Boundary scoring | Enable **Pack periodically (no clashes across box edges)** |
| Spacing / seed | `2.0 Å` / `3` |
| Pack around | `None` |
| Advanced | `Max restarts = 20`; `Max steps = 2000` |

Do not choose **Randomize** after setting the seed.

## Build and check the result

| Step | Action | Observable result |
| --- | --- | --- |
| 1 | Import and activate `BP-ARGON-01` | Details shows 1 Ar atom, 0 bonds, and no cell |
| 2 | Open **Disordered System**, add the active entry, and enter the fixed request | The panel shows 1 component row in `Copies` mode with 8 requested copies |
| 3 | Choose **Build** | A result entry named `Periodic argon` is created immediately; **Activity** shows the packing job as running |
| 4 | If a live progress line appears, inspect it while the run continues | Progress is emitted about every `75 ms` and shows placed/requested, steps, and worst overlap to 2 decimal places; the fixed 8-Ar run may finish before any line appears |
| 5 | After the run ends, inspect Details and **Activity**; also check the success notice if it remains visible | The notice reads `Packed 8 molecules into a disordered system`; **Activity** records **Build Disordered System** as `Completed`; Details shows the stored checks below |

The current GUI does not retain the detailed `PackReport` after the job ends. **Activity** retains
`Completed`, while the temporary success notice gives only the packed count. A missing live line does
not mean failure, and rerunning does not guarantee that the line will appear.

| Check | Expected visible state |
| --- | --- |
| Optional live progress, if shown | placed/requested, steps, and worst overlap rounded to 2 decimal places; it may show `8/8 placed` and `0.00 Å` |
| Temporary success notice, if still visible | `Packed 8 molecules into a disordered system` |
| Activity | **Build Disordered System** is `Completed`; this proves only that the workflow ended |
| Result name / formula | `Periodic argon` / `Ar8` |
| Atoms / stored bonds | 8 / 0 |
| Stored graph | 8 connected components |
| Cell | `16.000 x 16.000 x 16.000 Å`; `90.000 / 90.000 / 90.000°` |
| Region volume | `16 x 16 x 16 = 4096 Å^3` |

These stored checks establish the output composition and cell. The 8 atoms follow from
`1 atom per copy x 8 copies`, but neither atom count nor `Completed` proves full placement or a
clash-free structure.

## Engine reference (not retained by current GUI)

| Engine field | Deterministic reference |
| --- | --- |
| Requested / placed / unplaced | `8 / 8 / 0` |
| Convergence | `converged = true` |
| Maximum residual overlap | `0.000000 Å` |

These exact values are the deterministic engine reference for the fixed request, not a mandatory GUI
acceptance gate. The current GUI cannot prove these report fields after completion: it shows overlap
to 2 decimal places only if live progress appears and does not retain the detailed `PackReport`.

## Amount modes and mixtures

| Amount mode | Conversion for each component row |
| --- | --- |
| **Copies** | Uses the integer count entered for that row |
| **Density (g/cm^3)** | Converts that row's density using its template mass and the full region volume |
| **Concentration (mol/L)** | Converts that row's molar concentration using the full region volume |

The selected mode applies to all rows, but every row is converted independently against the full
region volume. Multiple rows are not normalized into a total density, mole fraction, or mixture
ratio, and the workflow does not check total charge. For a controlled mixture, determine the integer
count for every component first and enter the counts with **Copies**.

## Periodic scoring and the result cell

**Pack periodically** applies minimum-image boundary scoring to the spacing penalty across opposite
faces of the box. For a `Box`, enabling periodic packing also makes the engine write the box cell to
the result even when the explicit output-cell checkbox is off. **Use the region as the result's
simulation cell** makes that output-cell request explicit. This example enables both options so the
boundary-scoring and output-cell intent are clear.

Each component is translated and rotated as a rigid body, preserving its internal coordinates and
bonds. Spacing is an element-independent distance threshold between different rigid bodies. The
packing penalty is not a force field, potential energy, or statistical ensemble.

## Complete, partial, stopped, and timed-out results

- A step- or restart-limited run can end unconverged, a run can time out, and pressing Esc requests a
  stop while retaining the structure packed so far. The current GUI cannot distinguish all of these
  report states post hoc from **Activity** and Details alone.
- If the interface explicitly reports a stop, timeout, partial result, or fewer than 8 packed copies,
  do not use that entry as the fixed reference. Correct any request error before running again.
- Before any downstream simulation, export the structure or use an appropriate analysis tool to check
  composition, cell, molecular integrity, and every periodic minimum-image distance independently.
- For a formally auditable placed/unplaced/converged report, wait for product support that persists
  `PackReport` or use a validation path that outputs that report. Do not infer it from GUI state.
- Choosing **Cancel** before **Build** discards only the panel draft and is not a packing result.

## Recover to a known state

| Symptom | Recovery | Check before continuing |
| --- | --- | --- |
| The input is not monatomic Ar or the wrong component is selected | Cancel the panel, reimport the fixture, and activate the fresh entry | Details shows 1 Ar atom, 0 bonds, and no cell |
| A field is wrong before building | Choose **Cancel**, reopen the panel, and restore every fixed value | 1 row, 8 copies, the 16 Å cube, both options, 2.0 Å, seed 3, and no obstacle agree |
| Packing started with a wrong field | Press Esc, retain the result only for diagnosis, and start again from a fresh input | Only a new result made from the fixed request is eligible |
| No live progress line appeared | No recovery is needed for that reason alone; inspect Activity and the stored output | Remember that the line is optional and another run may also omit it |
| The run was explicitly partial, stopped, or timed out, or a notice reports fewer than 8 packed molecules | Keep the entry only for diagnosis; correct the request if needed before running again | Do not infer missing report fields from the retained structure |
| The stored formula, counts, components, or cubic cell differ | Rerun with both periodic and result-cell options enabled | Details shows `Ar8`, 8 atoms, 0 bonds, 8 components, 3 lengths of 16.000 Å, and 3 angles of 90.000° |
| Exact convergence or placement classification is needed | Use a validation path that outputs an auditable report, or wait until the GUI persists `PackReport` | Do not substitute Activity, the notice, or atom count |

Do not infer clash freedom or complete placement from the entry's presence, atom count, a transient
notice, or the Activity state alone.

## Scientific limits

This run checks rigid-body spacing optimization for one fixed request and seed. It does not assign a
force field, calculate energy, minimize intramolecular geometry, equilibrate the system, or sample an
ensemble. `converged` and `0.000000 Å` describe only this engine's packing penalty and residual-overlap
metric for the deterministic reference; an optional live line rounds overlap to 2 decimal places.
None of these values proves physically meaningful density, atomic radii, general clash freedom, or
simulation stability.

Before dynamics or statistical sampling, independently review composition, charge, cell, every
periodic minimum-image distance, molecular integrity, topology, and force-field parameters, then
perform an appropriate minimization and equilibration protocol.

## Related pages

- [Work with periodic cells and supercells](../periodic-cells-and-supercells/)
- [Import, fetch, and sketch structures](../../projects-structures/import-fetch-sketch/)
- [Edit and export structures](../../projects-structures/edit-and-export/)
