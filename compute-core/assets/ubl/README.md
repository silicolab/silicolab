# Bundled ubiquitin-like (UBL) templates

Canonical UBL structures used by `compute-core` for structural ubiquitination /
SUMOylation / NEDDylation (`workflows::ptm::ubiquitinate_protein`). Each file is
embedded at compile time via `include_str!` and parsed by the PDB reader into a
`Structure` with a biopolymer overlay.

Cleaning applied to every file: a single protein chain only; waters, ions, and
other HETATM groups dropped; hydrogens removed (heavy-atom template); `CRYST1`
and `CONECT` records dropped so bonds are inferred non-periodically. Each ends in
its resolved C-terminal di-glycine carrying the terminal `OXT`, the carboxyl that
condenses with a target lysine NZ.

| File            | Source (RCSB)        | Selection                          | Residues |
|-----------------|----------------------|------------------------------------|----------|
| `ubiquitin.pdb` | 1UBQ                 | chain A                            | 76       |
| `sumo1.pdb`     | 2N1V (NMR)           | model 1, chain A (mature SUMO1)    | 97       |
| `nedd8.pdb`     | 1NDD                 | chain B (resolves the Gly-Gly tail)| 76       |

Note on NEDD8: 1NDD chain A truncates at Arg74 (the C-terminal Gly-Gly is
disordered), so chain B — the conformer that resolves Gly75-Gly76 with its
`OXT` — is bundled instead, as the attachment requires the diGly C-terminus.

Source coordinates are from the RCSB Protein Data Bank (rcsb.org); PDB data are
released into the public domain.
