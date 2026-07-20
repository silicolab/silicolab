---
title: Edit and export structures
description: Commit Structure Editor changes explicitly, then export the intended entries with a suitable format and file layout.
sidebar:
  order: 6
---

## Edit the current entry

Double-click the target entry and confirm that it becomes current in the central
viewport. Then choose **Edit > Edit Structure...** to open
**Edit Structure**.

Structure Editor can:

- modify parameters of an existing periodic cell while preserving fractional
  positions and updating Cartesian coordinates;
- switch coordinate editing between **Fractional** and **Cartesian**, and
  **Wrap into cell**;
- add or delete atoms and edit element, charge, and coordinates;
- recompute, add, or delete bonds and choose Single, Double, Triple, or Aromatic
  bond types.

It cannot create or remove a cell, edit chain or residue metadata, optimize
geometry, or establish chemical plausibility. Cell controls appear only for a
structure that already has a periodic cell.

**Recompute Bonds** uses distance heuristics. **Wrap into cell** also recomputes
bonds after moving the coordinates. In both cases, inspect connectivity and
bond types before applying; the result is not validated valence or bond order.

> **Manual bond caution:** **Atom A** and **Atom B** use zero-based indices,
> while the atom table starts at 1. Do not copy a table row number directly into
> either field. Check the on-screen Preview for the intended elements and atom
> pair before adding the bond.

## End the editor session explicitly

Every editor change is previewed in the live viewport. Treat that preview as a
draft, not a committed or scientifically validated structure. **Undo** and
**Redo** inside Structure Editor navigate only the current editor draft.

| Exit | Result | Check |
| --- | --- | --- |
| **Apply** | Commits the whole editor session as one project-history step and closes the editor | Temporary status says `Applied structure edits`; the edited structure remains visible |
| **Cancel** | Restores the structure from before the editor opened and closes the editor | Temporary status says `Edit canceled`; the original structure returns |

After Apply, the main **Undo** reverses the entire editor session in one step and
**Redo** reapplies it. Before saving the project or leaving it, always choose
Apply or Cancel and confirm that the editor closes. Do not rely on Save Project
or a leave confirmation to decide the fate of an open preview.

For a persistent checkpoint after Apply, choose **File > Save Project** and
confirm the temporary `Saved project <name>` status while it is visible. Waiting
or checking Console is not equivalent to a successful explicit save.

## Separate project saving from structure export

| Action | Destination | It does not do |
| --- | --- | --- |
| **Save Project** | SilicoLab's project databases and state under the project root | It does not create a standalone exchange file |
| **Export...** | An external file or folder you choose | It does not replace project persistence and does not export only highlighted viewport atoms |

An important edit can require both decisions: first save the project checkpoint,
then create an external file when another program or workflow needs one.

## Set entry scope before Export

Export reads entries, never atom selection. **Selected (N)** contains the entry
rows selected in Entries plus every member of a selected group. This selection
is captured when the dialog opens.

1. Highlight the intended entries or groups in Entries.
2. Choose **File > Export...**.
3. In **Export Structures**, choose and verify **Selected (N)**,
   **Active structure (N)**, or **All in project (N)**.
4. Check the previewed names before choosing format and layout.

Opening Export from an entry's context menu keeps the full sidebar selection
when that entry is already selected. Opening it from an unselected entry narrows
the captured selection to that entry. In all cases, the scope counts and names
inside Export Structures are authoritative.

## Choose format and file layout

Writable formats are **XYZ**, **CIF**, **MOL2**, **PDB**, and **PDBQT**.

- One entry writes to one file.
- Several entries can use **Combine into one file** only for XYZ or MOL2.
- CIF, PDB, and PDBQT use **One file per structure** for several entries. The
  disabled combine option explains why the selected format is single-structure.
- Several PDB structures can still be exported successfully as separate files;
  only combining them into one PDB is refused.

For One file per structure, choose a destination folder. SilicoLab plans unique,
sanitized filenames. If planned files already exist, it asks
**Overwrite existing files?** before replacing them. A single or combined file
uses the native save chooser, which handles an existing filename itself.

On completion, temporary status reports the number of structures and the
destination. For separate-file export, it also reports the failure count and
first error if only some files could be written. Verify the destination files;
do not expect Console or Output to preserve the success message.

## Expect information loss across formats

Format conversion may omit or reinterpret bonds, charges, periodic cells, chain
and residue metadata, identifiers, or other source information. A successful
write is not evidence of a lossless conversion. Reopen the result in its
receiving program and verify atom count, elements, coordinates, connectivity,
cell, charges, and the metadata the next workflow requires.

> **Current graphical round-trip limit:** A combined XYZ or MOL2 file can hold
> every exported structure, but **File > Open File...** currently imports only
> the first identifier-less record. When each structure must be reopened and
> checked in SilicoLab, choose **One file per structure** and open the files
> separately.

## Related pages

- [Orient and style structures](../view-and-style/)
- [Understand entries, groups, and action scope](../entries-and-groups/)
- [Select entries, atoms, and sequence residues](../selection-and-sequence/)
- [Understand project directories, save checkpoints, and reopen behavior](../projects-and-workspaces/)
