---
title: Build nanosheets and annotate building blocks
description: Build the fixed periodic graphene sheet and understand the scope of Building Block Authoring.
sidebar:
  order: 7
---

## Goal

Build the default `4 x 4 x 1` periodic graphene structure from an empty project. The current result
must contain 32 atoms, 48 stored bonds, formula `C32`, one connected component, and the fixed cell
described below.

This page also explains **Building Block Authoring**. That tool annotates the entire active entry for
the reticular workflow; it does not extract the selected atoms and is not an input to Nanosheet
Builder.

## Build the fixed graphene structure

Open **Launch**, expand **Structure Builder**, and choose **Nanosheet Builder**. Enter or confirm every
value explicitly:

| Setting | Value |
| --- | --- |
| Name | `Nanosheet` |
| Type | `Honeycomb (A/B)` |
| Preset | `Graphene` |
| Sublattice A / B | `C / C` |
| Lattice a | `2.46 Å` |
| Buckling | `0 Å` |
| Interlayer spacing (A) | `12 Å` |
| Supercell | `4 x 4 x 1` |

For this zero-buckling graphene structure, **Interlayer spacing (A)** is the c-direction separation
between adjacent periodic images. It is not the thickness of a finite sheet.

1. Choose **Preview**. Inspect the periodic draft without treating it as an accepted entry.
2. Recheck all fixed values, then choose **Build**.
3. Confirm that a new `Nanosheet` entry is active and that **Activity** records **Nanosheet Builder**
   as `Completed`.
4. Inspect Details, the structure summary, and the stored bond graph. Continue only when all values
   below agree.

| Check | Expected value |
| --- | --- |
| Formula / atoms / stored bonds | `C32` / 32 / 48 |
| Stored graph | 1 connected component |
| Cell lengths | `9.840 x 9.840 x 12.000 Å` |
| Cell angles | `90 / 90 / 60°` |
| Stored charge | `0.0` on every atom |
| Built-geometry bond lengths / angles | `1.42 Å / 120°` |

If any value differs, stop and rebuild from the fixed parameters. Viewport appearance, representation,
and unit-cell wireframe visibility do not change the stored values.

## Interpret the periodic cell

Nanosheet Builder creates a periodic cell in all three directions. For this zero-buckling graphene
sheet, `c = 12 Å` is the separation between adjacent periodic images. It is not the thickness of a
finite sheet, an outer edge, or an edge-passivation distance.

Use the `4 x 4` in-plane repeats for this connectivity checkpoint. The stored graph can hold only one
bond for a given pair of atom indices. A `1 x 1` graphene cell therefore cannot represent the three
contacts made by the same atom pair through different periodic images. It can show the primitive
cell, but it cannot be used for this page's 48-bond, one-component checkpoint.

The other Honeycomb, transition-metal dichalcogenide, and graphitic carbon nitride presets are outside
this page's quantitative scope. Generation alone does not validate arbitrary element choices,
valence, bonding, material identity, or chemical suitability. New atoms in those builds also start
with stored charge `0.0`.

## Annotate a building block

Keep the structure that you intend to author as the active entry. Open **Launch**, expand
**Structure Builder**, and choose **Building Block Editor**. The **Building Block Authoring** panel
uses the entire current active entry as its structural input.

1. Set the **Label** and **Class** as needed. Class may be **Core**, **Linker**, or
   **Functional group**.
2. For each substitution site, choose two different atoms. The leaving atom must be a `Du` dummy
   atom directly bonded to the binding atom, and the binding atom must be non-`Du`. A selection can
   help locate those atoms, but it does not crop or reduce the structural input.
3. Choose **Save** only after confirming that the whole active entry is the intended building block.
   Save serializes the entire entry and the site annotations whose atom indices are in range to an
   SLF file at the selected location.

The current Save check excludes out-of-range atom indices but does not verify that the two atoms are
different, have the required `Du` / non-`Du` identities, or share a direct bond. Confirm every site
yourself before saving. Do not use two arbitrary graphene carbon atoms as a substitution site. When
the SLF is later loaded for a reticular build, the atom marked as leaving is removed from the
component; incorrect leaving metadata can therefore remove the wrong atom and produce an invalid
building block.

The saved text is injected only into the current in-memory reticular draft. It does not register a
persistent component library for other projects or later restarts. Nanosheet Builder does not consume
the building block, and saving it does not change the graphene draft.

## Recover to a known state

| Symptom | Recovery | Check before continuing |
| --- | --- | --- |
| A graphene parameter changed | Restore Honeycomb Graphene, C/C, `2.46 Å`, `0 Å`, `12 Å`, and `4 x 4 x 1`, then preview again | Every fixed input agrees |
| The preview state is uncertain | Choose **Cancel** before Build and reopen the builder | No result entry has been accepted |
| A `1 x 1` or other incorrect sheet was built | Start again in a clean writable project | The new entry matches all fixed graphene checks |
| Supercell expansion changed the active result | Rebuild the unexpanded graphene structure rather than inferring its earlier state | 32 atoms, 48 bonds, and the original cell are restored |
| Building Block Authoring was opened for the wrong entry | Choose **Cancel** before **Save**; if a file was already saved, do not use it as a nanosheet input | The nanosheet remains unchanged |
| No valid `Du`-binding bond exists | Choose **Cancel**; do not save or import the SLF. First create the correct dummy-site model or choose a suitable building block | Every site has a distinct `Du` leaving atom directly bonded to a non-`Du` binding atom |

Stop whenever formula, counts, cell, connectivity, bond geometry, or active-entry identity is
uncertain. Do not continue from an expanded or incorrectly parameterized result.

## Scientific limits

The graphene result is periodic and has no finite edges, edge termination, passivation, or defect
model. The `12 Å` spacing does not establish negligible interaction between periodic images. The
builder does not assign validated partial charges or force-field parameters, minimize the geometry,
optimize stacking, or assess phonons or dynamic stability. The preset name, `0.0` charges, `1.42 Å`
bonds, and `120°` angles do not by themselves establish stability, synthetic accessibility, or
simulation readiness. Validate the model and perform any required relaxation before downstream use.

## Related pages

- [Build reticular structures](../reticular-structures/)
- [Work with periodic cells and supercells](../periodic-cells-and-supercells/)
- [Edit and export structures](../../projects-structures/edit-and-export/)
