# Third-party notices

SilicoLab is available under GPL-3.0-or-later, with commercial licenses
available under separate written agreement from the SilicoLab copyright
holders. The repository records file-level licensing metadata using REUSE/SPDX
in `REUSE.toml` and `LICENSES/`.

Most Rust dependencies are permissively licensed (MIT / Apache-2.0 / BSD / Zlib
/ ISC / Unicode-style licenses) and impose no notice obligation beyond what
their own published packages already carry.

This file records third-party components whose notices must travel with a
binary redistribution of SilicoLab because the license is weak-copyleft,
font-specific, or because the component is vendored into this repository and
embedded into the binary. `Cargo.lock` is the exact dependency inventory for a
checkout; regenerate and review dependency-license reports when it changes.

---

## MPL-2.0 components

### xcx - exchange-correlation functionals for DFT

Copyright (c) 2026 Jiekang Tian and the xcx authors. Portions are derived from
libxc (<https://libxc.gitlab.io/>), Copyright (C) the libxc authors, including
M. A. L. Marques, X. Andrade, S. Lehtola, and M. Oliveira.

Pulled in transitively via hartree. SPDX: (MIT OR Apache-2.0) AND MPL-2.0. xcx is
licensed per file: its original code is MIT OR Apache-2.0, while files derived
from libxc remain under the Mozilla Public License, Version 2.0.

MPL-2.0 is file-level weak copyleft. It places no restriction on SilicoLab as a
whole, nor on your use of it; only modifications to the MPL-licensed source
files themselves must be released under MPL-2.0. SilicoLab links this code
unmodified. The corresponding source, including the MPL-licensed files and the
full license text, is published with the crate at <https://crates.io/crates/xcx>.

If you redistribute SilicoLab in binary form, keep this notice available and
ensure recipients can obtain that source, as required by MPL-2.0 section 3.2.
The full MPL-2.0 text is available at <https://www.mozilla.org/MPL/2.0/>.

---

## Bundled data assets

These third-party data files are vendored into this repository and embedded
into the binary, so the copyright and permission notices below must accompany
binary redistributions.

### CHARMM36 force field

Copyright (c) 2025 mackerell-lab. Licensed under the MIT License. A pruned
subset is bundled under `assets/forcefields/charmm36.ff/`; its upstream release
and retained scope are recorded in `assets/forcefields/charmm36.ff/VERSION`, and
the in-tree license is `assets/forcefields/charmm36.ff/LICENSE`. REUSE metadata
records this subtree as MIT.

### Liberation Sans embedded font

`assets/fonts/LiberationSans-Regular.ttf` (Liberation Fonts 2.1.5,
<https://github.com/liberationfonts/liberation-fonts>) is compiled into the
binary and embedded in exported charts. It is licensed under the SIL Open Font
License, Version 1.1; the full license text is vendored at
`assets/fonts/LiberationSans-LICENSE`. REUSE metadata records these files as
OFL-1.1.

### Default egui fonts

Some GUI font assets are provided by transitive egui/epaint packages and are
licensed under their published font licenses, including OFL-1.1 and Ubuntu Font
License terms. These licenses continue to apply to those font assets under both
the GPL and commercial licensing paths.

### RCSB Protein Data Bank templates

The bundled ubiquitin-like PDB templates under `compute-core/assets/ubl/` are
cleaned derivatives of RCSB Protein Data Bank coordinate files identified in
`compute-core/assets/ubl/README.md`. RCSB PDB data are released into the public
domain; REUSE metadata records the PDB coordinate files as
LicenseRef-Public-Domain.
