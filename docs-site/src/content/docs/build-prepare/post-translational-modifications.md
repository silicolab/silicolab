---
title: Apply structural post-translational modifications
description: Add a structural phosphate group to Ser20 in mapping-preserving hydrogenated ubiquitin, then check connectivity, counts, and scientific limits.
sidebar:
  order: 3
---

## Goal

Start from a fresh 602-atom ubiquitin heavy-atom entry, add 570 hydrogens in Console while preserving
residue-to-atom mapping, then use **Modify PTM** to attach a phosphate group to chain A Ser20.

The fixed count sequence is:

```text
602 / 608 / 76 -> 1,172 / 1,178 / 76 -> 1,177 / 1,183 / 77
```

The final result has two chain records and one connected component.

## Fixed sample and required input

Download [`PS-UBIQUITIN-01`](../../samples/ubiquitin.pdb). It is a cleaned heavy-atom template of
chain A from 1UBQ, not the raw database deposition or a complete biological assembly. Waters, ions,
hydrogens, other chains, `CRYST1`, and `CONECT` have been removed.

Fresh means an imported entry on which no preparation or modification operation has run. Import the
file, explicitly make it the active entry, and verify:

| Check | Starting value |
| --- | --- |
| Atoms / bonds / residues | `602 / 608 / 76` |
| Formula | `C378N105O118S` |
| Chain / cell | Chain A / `none` |

Non-glycan PTM attachment requires a displaceable explicit hydrogen on the target heavy atom. This
workflow therefore uses the mapping-preserving Console command `hydrogen add`, not the GUI protein
preparation result whose mapping does not include the added hydrogens.

## Phosphorylate A:Ser20

| Step | Action | Observable result |
| --- | --- | --- |
| 1 | Choose **File > Open File...**, import a fresh `ubiquitin.pdb`, and explicitly activate that entry | Details shows 602 atoms, 608 bonds, 76 residues, formula `C378N105O118S`, and cell `none` |
| 2 | Open **Console**, enter `hydrogen add`, and run it once | The Console command transcript reports `added 570 hydrogen(s)`; the active entry is hydrogenated in place with residue mapping preserved. It now has 1,172 atoms, 1,178 bonds, 76 residues, formula `C378H570N105O118S`, and cell `none` |
| 3 | Keep that exact 1,172-atom intermediate active. Open **Launch**, expand **Structure Editing**, and choose **Modify PTM** | The **Modify Protein (PTM)** panel opens with the mapping-preserving intermediate as the host |
| 4 | Set **Modification** to **Phosphorylate**, **Chain** to `A`, and **Residue #** to `20` (Ser20). Leave **Result name** blank for automatic naming | The panel shows `Phosphorylate`, chain A, and residue 20; no result has been created yet |
| 5 | Choose **Apply** | A new result is created and activated. The transient status notice reports `Phosphorylate applied` with a project-dependent entry number; **Activity** records **Modify Protein (PTM)** as `Completed` |
| 6 | Inspect the active result | It has 1,177 atoms, 1,183 bonds, 77 residue records, 2 chain records, 1 connected component, formula `C378H571N105O121PS`, and no periodic cell |

Do not identify the result by its entry number or automatic name. Both depend on the current project.
Use the input state, target site, composition, and counts instead.

## Verify the result and junction

| State | Atoms | Bonds | Residues | Formula | Other structural state |
| --- | ---: | ---: | ---: | --- | --- |
| Fresh heavy-atom input | 602 | 608 | 76 | `C378N105O118S` | Chain A; no cell |
| Mapping-preserving `hydrogen add` intermediate | 1,172 | 1,178 | 76 | `C378H570N105O118S` | 570 hydrogens added in place; no cell |
| A:Ser20 phosphorylated result | 1,177 | 1,183 | 77 | `C378H571N105O121PS` | Two chain records; one connected component; no cell |

The 77 records are 76 host amino-acid residues plus one nonstandard `PO4` modifier residue. The two
chain records are the host chain and modifier fragment, not two protein subunits. A covalent `OG-P`
junction connects Ser20 to the phosphate fragment and joins them into one connected component.

The generated geometry also contains a short host-fragment contact of about `1.242 Å`, between an
added hydrogen mapped to `A:Glu18` and modifier atom `PO4 O1P`. This distance remains after excluding
host-fragment pairs separated by one, two, or three covalent bonds. It is not a force-field definition
of a nonbonded contact and does not establish a clash-free structure; it signals unresolved overlap risk.

## Supported modification families and anchors

These are structural attachment choices, not biological site predictions.

| Modification | Eligible residue and anchor | Selection boundary |
| --- | --- | --- |
| **Phosphorylate** | Ser `OG`, Thr `OG1`, Tyr `OH`, or His `NE2` | The panel uses `NE2` for His; this workflow uses Ser20 `OG` |
| **Acetylate** | Lys `NZ`, or backbone `N` of the selected chain's first residue | With the N-terminal option enabled, the selected residue must be the first residue of that chain |
| **Methylate** | Lys `NZ` or Arg `NH1` | Mono, di, and tri degrees are available; the residue must match the anchor |
| **Lipidate** | Cys `SG` for palmitoyl, farnesyl, or geranylgeranyl; backbone `N` of chain-initial Gly for myristoyl | A myristoyl target must be Gly and the first residue of the selected chain |
| **Ubiquitinate** | Host Lys `NZ` | Ubiquitin, SUMO, or NEDD8 can be selected; the host anchor remains Lys `NZ` |
| **Glycosylation** | Asn `ND2` for N-linked; Ser `OG` or Thr `OG1` for O-linked | The anchor residue determines the linkage |

**Modify PTM** accepts a chain identifier and integer residue number. It cannot select a residue ID
with an insertion code. Do not drop an insertion code and substitute the ordinary residue with the same
number.

## Recover to a known state

| Symptom | Recovery |
| --- | --- |
| Imported counts are not `602 / 608 / 76`, or the active entry is uncertain | Reimport the sample, explicitly activate the new entry, and verify the counts and `C378N105O118S` together |
| `hydrogen add` ran on the wrong entry or did not produce `1,172 / 1,178 / 76` | Stop using that entry; reimport and activate a fresh source, verify the starting state, then run the command once |
| Apply reports `modification failed`, cannot find the residue, or finds no displaceable anchor hydrogen | Rebuild the exact mapping-preserving intermediate, activate it, and recheck chain A and residue 20 |
| A result was created with the wrong family, chain, or residue | Do not continue from it; reactivate the checked 1,172-atom intermediate and repeat the settings |
| The target residue has an insertion code | Stop this GUI route and leave the intermediate unchanged |

## Scientific limits

The generated PTM geometry is idealized and unminimized. `Completed`, the formula, the counts, and the
stored `OG-P` bond establish only that SilicoLab performed the structural attachment. They do not assign
a biological modification state, enzyme specificity, occupancy, final charge, or a complete force-field
topology.

Before parameterization or simulation, verify the intended biological modification, chemical junction,
steric clashes and other overlaps, protonation, and charge. Build and review the complete junction
topology and force-field parameters, then minimize or otherwise relax the whole structure. The generated
entry is not MD-ready as-is.

## Related pages

- [Add hydrogens and prepare a protein](../protein-preparation/)
- [Build glycans and attach them to proteins](../glycans-and-glycosylation/)
- [Select entries, atoms, and sequence residues](../../projects-structures/selection-and-sequence/)
- [Understand entries, groups, and action scope](../../projects-structures/entries-and-groups/)
- [Edit and export structures](../../projects-structures/edit-and-export/)
