---
title: Import, fetch, or sketch structures
description: Choose file import, PDB ID fetch, or 2D sketch building, then inspect the resulting project entries and failure states.
sidebar:
  order: 3
---

## Choose an input path

| Path | Use it when | Completion check |
| --- | --- | --- |
| File import | You already have a supported local structure file | A new entry appears in Entries; an identified PDB also creates a group |
| PDB ID fetch | You know a classic four-character PDB ID and network access is available | The PDB is cached under the current project's `structures/` directory and then imported |
| 2D sketch | You are starting from SMILES or a hand-drawn structure | A new current entry is created only after **Build (Save as New)** |

Use a persistent project when the imported source, cache, and resulting entries
need to survive the session.

## Import a local file

Choose **File > Open File...** and select one supported structure file. SilicoLab
can read XYZ, CIF, MOL2, SLF, GRO, PDB, and PDBQT files. A successful import adds
an entry, makes the first imported structure current, clears the previous atom
selection, and posts temporary status feedback.

### Check PDB grouping with a two-model fixture

Download the [synthetic truncated two-model fixture](../../samples/6a5j-two-model-ui-fixture.pdb).
Each model contains only the N and CA atoms of one GLY residue. This file is not
the scientific 6A5J model and must not be used for structural interpretation or
calculation.

1. Open the fixture with **File > Open File...**.
2. In Entries, expand `SOLUTION NMR STRUCTURE OF SMALL PEPTIDE`.
3. Confirm that it contains `6A5J (model 1)` and `6A5J (model 2)`.
4. Confirm that model 1, not the group highlight, is the current entry in the
   central viewport.

PDB files that carry a deposition identifier use the deposition title as a
group name. A single-model deposition still gets a group with one
identifier-named entry. A multi-model deposition gets one entry per model, but
only the first model becomes current. When title metadata is absent, SilicoLab
falls back to the identifier; a headerless PDB can use the uppercase filename
stem as its fallback identifier.

Files without a deposition identifier, including ordinary XYZ and MOL2 files,
are imported as ungrouped entries whose names come from parsed title or file
information.

> **Current limit:** The parsers can recognize multiple XYZ or MOL2 records, but
> the graphical import path keeps only the first record when the file has no
> deposition identifier. It does not warn that later records were omitted.
> Split the records into separate files before importing when every record is
> required.

### Import several files by drag and drop

The native Open File chooser accepts one file. Drag and drop can submit several
files together.

| Result | What remains in the workspace | Recovery |
| --- | --- | --- |
| Every file succeeds | Every imported document remains; temporary status reports the opened count | Inspect the new current entry and Entries before continuing |
| Some files fail | Successful imports remain; temporary status reports both opened and failed counts | Correct and resubmit only the failed files to avoid duplicates |
| Every file fails | No new entry is created; status shows the first open error | Correct the first reported format, extension, or permission problem, then retry |

## Fetch a structure by PDB ID

1. Choose **File > Fetch from PDB ID...**.
2. Enter a classic PDB ID such as `1ubq` and choose **Fetch**.
3. On success, confirm that the dialog closes and the imported structure appears
   in Entries.

SilicoLab trims the ID, validates it, converts it to uppercase, and stores the
file as `structures/1UBQ.pdb` under the current project. A classic PDB ID must
contain exactly four ASCII letters or digits and start with a digit. For example,
`abcd` remains in the open dialog and produces a temporary error beginning
`Fetch failed:`.

| Situation | Behavior and recovery |
| --- | --- |
| Invalid ID, network error, or write failure | The dialog and input remain open. Correct the ID, network access, or project write access and retry |
| The uppercase ID is already cached | SilicoLab reuses the current project's cached file, then imports it normally |
| Download succeeds but parsing fails | The fetch dialog has already closed and normal file-open failure feedback appears. Repair the cached file or retry with the correct ID |

The cache is project-specific. A file under another project's `structures/`
directory does not show that the current project fetched it.

## Build a structure from a 2D sketch

1. Choose **File > Sketch Molecule...**. The **Build (Save as New)** button is
   disabled while the sketch is empty.
2. Enter a SMILES string and choose **Import**. On success, the canvas is
   replaced and the sketch window reports `Imported SMILES (<N> atoms)`. No
   project entry is created yet.
3. Inspect or edit the drawing, enter a **Title**, and choose
   **Build (Save as New)**.
4. Confirm that the sketch window closes, a new entry becomes current, and its
   generated 3D coordinates appear in the central viewport.

If SMILES parsing fails, the sketch window reports `SMILES error:` and keeps the
previous drawing. Correct the text and import again; do not mistake the retained
drawing for the failed input. Choose **Cancel** to close the sketch without
creating an entry.

Build adds inferred hydrogens, generates initial 3D coordinates, and attempts
UFF relaxation only for supported elements. A structure with unsupported
elements can still be created with unrelaxed coordinates.

**Scientific boundary:** Importing SMILES, drawing bonds, or building 3D
coordinates is not chemical repair, conformer search, scientific validation, or
proof that a structure is ready for research. Independently inspect composition,
formal charges, connectivity, stereochemistry, and geometry before calculation.

## Related pages

- [Understand entries, groups, and action scope](../entries-and-groups/)
- [Select entries, atoms, and sequence residues](../selection-and-sequence/)
- [Understand project and workspace persistence boundaries](../projects-and-workspaces/)
