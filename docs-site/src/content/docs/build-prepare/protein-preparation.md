---
title: Add hydrogens and prepare a protein
description: Add missing hydrogens to the ubiquitin sample, inspect the new entry, and use the Console path when residue-to-atom mapping is required.
sidebar:
  order: 1
---

## Goal

Create a separate hydrogen-completed entry from the 602-atom ubiquitin heavy-atom template. Check the
change from `602 / 608 / 76` to `1,172 / 1,178`, confirm that 570 hydrogens were added, and understand
the residue-to-atom mapping limit of the GUI result.

Use the Console command `hydrogen add` on a fresh source entry only when a later operation needs the
added hydrogens to retain residue ownership.

## Fixed sample

Download [`PS-UBIQUITIN-01`](../../samples/ubiquitin.pdb). This is a cleaned heavy-atom template of
chain A from 1UBQ, not the raw database deposition or a complete biological assembly. Waters, ions,
hydrogens, other chains, `CRYST1`, and `CONECT` have been removed.

Import the file, then double-click its entry to make it the active entry. Before continuing, verify all
of these values together:

| Check | Starting value |
| --- | --- |
| Atoms / bonds / residues | `602 / 608 / 76` |
| Formula | `C378N105O118S` |
| Chain / cell | Chain A / `none` |

## Prepare the protein in the GUI

Open **Launch**, expand **Molecular Dynamics**, and choose **Prep Protein**. Keep
**Add missing hydrogens** enabled, then choose **Prepare**.

| Step | Action | Observable result |
| --- | --- | --- |
| 1 | Choose **File > Open File...**, import `ubiquitin.pdb`, and explicitly activate the imported entry | Details shows 602 atoms, 608 bonds, and formula `C378N105O118S`; Sequence contains 76 residues; the cell is `none` |
| 2 | In **Launch > Molecular Dynamics**, choose **Prep Protein** | The **Protein Preparation** panel opens with **Add missing hydrogens** enabled; no result has been created yet |
| 3 | Choose **Prepare** | A transient status notice reports `Protein prepared: added 570 hydrogen(s) (new entry)`; **Activity** records **Protein Preparation** as `Completed`; a new result is created and activated |
| 4 | Inspect the new result in Details, and confirm that the source remains a separate entry | The result has 1,172 atoms, 1,178 bonds, formula `C378H570N105O118S`, and cell `none`; the source was not replaced in place |

`Completed` means that the software operation ended. It is not a scientific validation of the
prepared structure.

## Compare input and output

Protein metadata includes a residue-to-atom mapping, called `biopolymer mapping` in the current
implementation. It records which residue owns each mapped atom.

| State | Stored checks | Residue-to-atom mapping |
| --- | --- | --- |
| Heavy-atom input | `602 atoms / 608 bonds / 76 residues`; `C378N105O118S`; cell `none` | Covers all 602 input atoms across 76 residues |
| GUI result | `1,172 atoms (+570) / 1,178 bonds (+570)`; `C378H570N105O118S`; cell `none` | Copies the original residue records and the mapping for the first 602 atoms, but does not extend the mapping to the 570 added hydrogens |

Each added hydrogen forms one new bond, so both the atom count and bond count increase by 570. This
checks composition and the connectivity delta only. It does not check steric overlap or relax the
geometry.

The **Prep Protein** panel currently performs generic valence-based hydrogen completion and stores the
result as a new entry. Keep this result when a separate hydrogen-completed structure is sufficient and
the missing mapping for the new hydrogens will not affect later work.

## Preserve residue mapping with Console

If a later structural operation needs residue ownership for the added hydrogens, start again from a
freshly imported `PS-UBIQUITIN-01`. Do not run the command on the 1,172-atom GUI result.

| Step | Action | Observable result |
| --- | --- | --- |
| 1 | Reimport the sample and explicitly activate the fresh source entry | Details and Sequence return to `602 / 608 / 76`, `C378N105O118S`, and cell `none` |
| 2 | Open **Console**, enter `hydrogen add`, and run it | The Console command transcript reports `added 570 hydrogen(s)`; the active entry is modified in place instead of creating another preparation result |
| 3 | Inspect Details and Sequence again | The active entry has `1,172 / 1,178 / 76`, formula `C378H570N105O118S`, and cell `none`; each new hydrogen is mapped to the residue of its bonded heavy atom |

This command preserves the mapping needed by structural operations such as the PTM route on the next
page. It does not make the chemical decisions omitted by generic hydrogen completion.

## Recover to a known state

If the wrong entry was prepared or modified, stop using that result. Reimport the sample, explicitly
activate the new import, and continue only after `602 / 608 / 76`, `C378N105O118S`, and cell `none`
appear together.

| Symptom | Recovery |
| --- | --- |
| The Protein Preparation panel has the wrong input or option | Choose **Cancel**, then reimport and activate the sample before reopening **Prep Protein** |
| The GUI created a result from the wrong source | Leave that result unused and restart from a new import; the source was not overwritten |
| `hydrogen add` ran on the wrong entry | Stop using that entry; reimport the sample rather than assuming Undo recreated the fixed starting state |
| Console reports `hydrogen add requires an open entry`, or the counts differ | Activate a fresh import and recheck all starting values before running the command |

## Scientific limits

Both paths perform generic valence-based hydrogen completion. They do not select protonation states,
histidine tautomers, terminal states, disulfide bonds, or charges, and they do not add missing heavy
atoms. They also do not perform a protein-specific clash check or geometry minimization.

Before parameterization or simulation, inspect missing heavy atoms, residue chemistry, protonation,
histidine states, termini, disulfides, charge, steric contacts, and force-field topology, then minimize
or otherwise relax the geometry as required. Atom counts, bond counts, a completed task, and preserved
residue mapping do not make the structure MD-ready.

## Related pages

- [Select entries, atoms, and sequence residues](../../projects-structures/selection-and-sequence/)
- [Understand entries, groups, and action scope](../../projects-structures/entries-and-groups/)
- [Edit and export structures](../../projects-structures/edit-and-export/)
