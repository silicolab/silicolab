---
title: Select entries, atoms, and sequence residues
description: Keep the current entry, sidebar selection, and atom or residue selection separate, then verify Sequence synchronization.
sidebar:
  order: 4
---

## Establish a reproducible sample

Download the [ubiquitin sample](../../samples/ubiquitin.pdb). It is a cleaned
1UBQ chain A heavy-atom template with 602 atoms and 76 residues. Waters, ions,
hydrogens, other chains, `CRYST1`, and `CONECT` have been removed. SilicoLab
therefore infers bonds without periodic boundaries. The sample is not the raw
deposition or a complete biological assembly.

1. Choose **File > Open File...** and open the sample.
2. Confirm that Entries contains group
   `UBIQUITIN (1UBQ chain A, heavy atoms)` and entry `UBIQUITIN`, and that the
   entry is current in the central viewport.
3. Open the bottom **Sequence** tab. Confirm that chain A and 76 residues appear.
4. Click residue `K` at sequence position 48. Confirm that Lys48, its nine heavy
   atoms in the viewport, and the Sequence counts highlight together.

This synchronized display checks selection behavior only. It does not establish
scientific completeness or suitability of the sample for calculation.

## Distinguish three selection scopes

| Scope | Meaning | Visible check |
| --- | --- | --- |
| Current entry | The one entry shown in the central viewport and used by ordinary single-structure tasks | Its tab is current and its structure is visible |
| Sidebar-selected entries or groups | The highlighted Entries scope used by Export and sidebar batch actions | Entry rows or group headers highlight without automatically changing the current entry |
| Selected atoms or residues | Atom indices inside the current entry; selecting a residue expands to that residue's atoms | Atoms highlight in the viewport and compatible Sequence rows show matching residue highlights and counts |

## Select or activate entries

| Action | Result |
| --- | --- |
| Single-click an entry | Selects its sidebar row without making it current |
| Double-click an entry | Selects it, opens or activates its tab, and makes it current |
| `Cmd`/`Ctrl`-click | Toggles an entry or group in the sidebar selection without changing the viewport |
| `Shift`-click | Selects the visible range from the current anchor to the target |

Collapsed group members and rows hidden by a search are not part of a Shift
range. Use the rows that actually highlight as the authoritative sidebar scope.

## Select atoms in the viewport

| Action | Atom-selection result |
| --- | --- |
| Click an atom | Replaces the atom selection with that atom |
| `Cmd`/`Ctrl`-click an atom | Toggles that atom in the current selection |
| Click empty viewport space | Leaves atom selection unchanged |
| **Selection > Clear Selection** or `Cmd/Ctrl+Shift+A` | Clears viewport and Sequence highlights without clearing the Entries sidebar selection |

Temporary status feedback reports the selected atom count or confirms that atom
selection was cleared. Atom selection applies only to the current entry and is
not an Export scope.

## Select residues in Sequence

| Action | Result |
| --- | --- |
| Click one residue | Replaces atom selection with all valid atoms in that residue |
| `Cmd`/`Ctrl`-click a residue | Toggles all atoms in that residue |
| `Shift`-click a second residue in the same chain | Selects the chain-local residue range from the anchor to the target |
| Drag across residue cells | Selects the residues touched by the drag; modifiers can add, toggle, or remove them |
| Double-click a chain rail | Replaces selection with all displayed residues in that chain |

Sequence is another view of the current atom selection, not a separate saved
selection list. Its header reports selected residue and atom counts, and the
viewport highlights the same atoms.

Sequence shows residue rows only when the current entry contains compatible
protein or nucleic-acid metadata for its present atom count. Otherwise it says
`No protein or nucleic-acid sequence metadata for the active structure.` This
message alone does not prove that file import failed; first verify the current
entry, structure type, and atom count.

## Recheck scope after importing or switching entries

A successful file import clears atom selection. Switching the current entry does
not guarantee an empty selection: indices that remain valid for the new
structure can survive, while out-of-range indices are removed.

After switching entries, inspect both the viewport and Sequence. Before an
operation whose scope matters, use **Clear Selection** and select the intended
atoms or residues again.

## Related pages

- [Import, fetch, or sketch structures](../import-fetch-sketch/)
- [Understand entries, groups, and action scope](../entries-and-groups/)
- [Orient and style structures](../view-and-style/)
