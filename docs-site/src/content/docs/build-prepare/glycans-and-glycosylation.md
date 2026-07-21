---
title: Build glycans and attach them to proteins
description: Build a standalone glycan in Console, then attach the same glycan to a fresh heavy-atom protein input with Modify PTM.
sidebar:
  order: 2
---

## Goal

Complete two independent workflows and inspect each result separately:

| Workflow | Starting point | Result |
| --- | --- | --- |
| Standalone glycan | A clean Scratch workspace with no file open and no existing entry selected | A five-residue standalone glycan named `Branched glycan` |
| Protein attachment | A freshly imported heavy-atom `PS-UBIQUITIN-01` | The same glycan attached N-linked at `A:Asn25` in a new entry |

The standalone glycan is not the protein input for the attachment workflow. Start the second workflow
from a new import of the protein sample.

## Build a standalone glycan

In a clean Scratch workspace, open **Console** and run:

```text
glycan Man(a1-3)[Man(a1-6)]Man(b1-4)GlcNAc(b1-4)GlcNAc --name "Branched glycan"
```

The Console command transcript reports `built glycan Branched glycan`, the new entry's
project-dependent number, `120 atoms`, and the normalized product notation. Do not use the entry
number to identify the result.

| Check | Result |
| --- | --- |
| Entry name | `Branched glycan` |
| SilicoLab-normalized IUPAC-condensed notation | `Man(a1-3)[Man(a1-6)]Man(b1-4)GlcNAc(b1-4)GlcNAc(b1-)` |
| Structure counts | 120 atoms, 129 bonds, 5 residues, 1 chain, 1 connected component |
| Formula | `C34H58N2O26` |
| Periodic cell | `none` |

SilicoLab accepts IUPAC-condensed-style input and serializes a normalized product string. The
standalone suffix `(b1-)` records a beta reducing end without a protein acceptor class. After N-linked
attachment, `(b1-N)` records the beta reducing end and N-linked acceptor class. Neither suffix names a
protein residue or provides a complete site-specific glycopeptide notation.

## Attach the glycan to ubiquitin

Download [`PS-UBIQUITIN-01`](../../samples/ubiquitin.pdb). It is a cleaned heavy-atom template of
chain A from 1UBQ, not the raw database deposition. Waters, ions, hydrogens, other chains, `CRYST1`,
and `CONECT` have been removed. The fresh input has 602 atoms, 608 bonds, 76 residues, formula
`C378N105O118S`, and no periodic cell.

Do not use a hydrogen-added, modified, or previously generated entry for this workflow.

| Step | Action | Observable result |
| --- | --- | --- |
| 1 | Choose **File > Open File...**, import the sample, and explicitly activate the new protein entry | Details shows 602 atoms and 608 bonds; Sequence shows chain A with 76 residues |
| 2 | Open **Launch**, expand **Structure Editing**, choose **Modify PTM**, and set **Modification** to **Glycosylation** | The panel shows target-residue, glycan, **Linkage**, and **Reducing end** controls, with the fresh protein as the input |
| 3 | Set **Chain** to `A`, **Residue #** to `25` (`A:Asn25`), **Linkage** to `N-linked`, and **Reducing end** to `Automatic`; enter `Man(a1-3)[Man(a1-6)]Man(b1-4)GlcNAc(b1-4)GlcNAc` under **Glycan (IUPAC)** | Before applying, confirm both chain `A` and residue `25`; this sample also contains Asn60 |
| 4 | Choose **Apply** | A new result is created and activated; the transient status notice reports `Glycosylation applied`, the result's project-dependent entry number, and `N-linked as Man(a1-3)[Man(a1-6)]Man(b1-4)GlcNAc(b1-4)GlcNAc(b1-N)`; **Activity** records **Modify Protein (PTM)** as `Completed` |
| 5 | Inspect the active result in Details | The result has 720 atoms, 736 bonds, 81 residue records, 2 chain records, 1 connected component, formula `C412H57N107O143S`, and no periodic cell |
| 6 | Export the active result as PDB and inspect its `LINK` record | The intended result joins glycan `C1 NAG B 1` to protein `ND2 ASN A 25`; its `CONECT` records encode the same stored bond |

Entry numbers vary with project contents. Identify the result by its input, normalized notation, counts,
and the exported `A:Asn25 ND2` to glycan `C1` junction, not by an entry number.

The formula is an atom-inventory check, not the formula of a fully protonated glycoprotein. It combines
the hydrogen-free protein with the explicitly hydrogenated glycan after removal of the reducing-end
`O1`/`HO1` leaving group. It does not define a complete protonation state or charge.

The 81 residue records are 76 host amino-acid residues plus five carbohydrate residues. The two chain
records are host chain A and glycan chain B, not two protein subunits. The `ND2-C1` junction makes the
product one connected component.

## Glycosylation anchors

The target residue constrains the protein-side anchor and linkage. A linkage choice that conflicts with
the residue fails instead of converting the residue into another anchor type.

| Linkage | Eligible residue | Protein anchor | Junction for a GlcNAc reducing end |
| --- | --- | --- | --- |
| N-linked | Asn | `ND2` | Asn `ND2` to glycan `C1` |
| O-linked | Ser | `OG` | Ser `OG` to glycan `C1` |
| O-linked | Thr | `OG1` | Thr `OG1` to glycan `C1` |

The N-linked implementation requires a GlcNAc reducing end. Other sugar classes can use a different
anomeric-carbon index, such as `C2` for sialic acid. Do not directly attach a reducing-end sialic acid
to a protein; choose a reducing-end sugar, linkage, and biological site supported by the intended model.

## Recover to a known state

| Symptom | Recovery |
| --- | --- |
| The standalone command reports a notation error | Paste the complete command from this page again; inspect the result only after the Console transcript reports 120 atoms and the full normalized notation |
| Apply reports `modification failed:` | Recheck the active input, `A:Asn25`, N-linked, Automatic, and the complete glycan string, then apply again |
| A result was created from the wrong input or settings | Reactivate a fresh 602-atom source, confirm 76 residues, and repeat from **Modify PTM** |
| Existing entries make the result number unexpected | Ignore the number and use the normalized notation, `720 / 736` counts, formula, and junction check |
| The attachment site is uncertain | Return to the fresh source, confirm chain A and residue 25, repeat the operation, and accept the intended route only when the exported PDB joins `C1 NAG B 1` to `ND2 ASN A 25` |

## Scientific limits

Both standalone and attached glycan geometries are idealized, unminimized starting structures. Their
notation, counts, and formula confirm the generated composition, not an equilibrium conformation or
experimental structure.

The documented attached structure contains a severe unrelaxed overlap: the shortest host-glycan
separation is about `0.402 Å`, between protein `A:Val17 CA` and glycan `B:NAG2 HN`. The same short
distance remains after excluding the direct `ND2-C1` junction and atom pairs separated by one, two, or
three covalent bonds. This distance is not a force-field definition of a nonbonded contact, but it is
enough to show that the generated attachment is not scientifically usable as-is.

`A:Asn25` is followed by Val26 and Lys27, so this `Asn-Val-Lys` site is a software-connectivity example,
not a usual `N-X-S/T` sequon. The selected site and stored junction do not establish site occupancy,
enzyme specificity, or biological feasibility.

Before parameterization or simulation, verify the biological site and glycan, chemical connectivity,
anomer and linkage stereochemistry, steric clashes, protonation, and charge. Build and review the full
junction topology and force-field parameters, then minimize or otherwise relax the complete geometry.

## Related pages

- [Add hydrogens and prepare a protein](../protein-preparation/)
- [Apply structural post-translational modifications](../post-translational-modifications/)
- [Select entries, atoms, and sequence residues](../../projects-structures/selection-and-sequence/)
- [Edit and export structures](../../projects-structures/edit-and-export/)
